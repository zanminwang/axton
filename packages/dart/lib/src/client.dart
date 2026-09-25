import 'connection.dart';
import 'actions.dart';
import 'live.dart';
import 'port.dart';
import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:isolate';
import 'dart:io';
import 'dart:math';
import 'package:ffi/ffi.dart';

typedef _CallNative = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _Call = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _FreeNative = Void Function(Pointer<Utf8>);
typedef _Free = void Function(Pointer<Utf8>);

class _MutationRollbackError extends Error {
  final Object submission;
  final Object rollback;
  _MutationRollbackError(this.submission, this.rollback);
  @override
  String toString() =>
      'mutation submission and rollback failed: $submission; $rollback';
}

void _nativeWorker(List<Object?> args) {
  final ready = args[0] as SendPort;
  try {
    final libraryPath = args[1] as String?;
    final library = libraryPath != null
        ? DynamicLibrary.open(libraryPath)
        : Platform.isIOS
        ? DynamicLibrary.process()
        : throw StateError('libraryPath is required outside iOS');
    final call = library.lookupFunction<_CallNative, _Call>('axton_call');
    final free = library.lookupFunction<_FreeNative, _Free>('axton_free');
    final port = ReceivePort();
    ready.send(port.sendPort);
    port.listen((dynamic raw) {
      if (raw == null) {
        port.close();
        return;
      }
      final message = raw as List;
      final reply = message[1] as SendPort;
      final input = (message[0] as String).toNativeUtf8();
      try {
        final output = call(input);
        try {
          reply.send(output.toDartString());
        } finally {
          free(output);
        }
      } catch (error) {
        reply.send(jsonEncode({'ok': false, 'error': error.toString()}));
      } finally {
        calloc.free(input);
      }
    });
  } catch (error) {
    ready.send(error.toString());
  }
}

/// Typed generated model APIs delegate to this generic native client.
class Client implements WritePort, MutatePort {
  final SendPort _worker;
  final Isolate _isolate;
  final int _handle;
  final String clientId;
  Future<void> _tail = Future<void>.value();
  LiveLane? _live;
  final _channels = StreamController<void>.broadcast(sync: true);
  Future<void>? _syncing;
  Future<void>? _tasks;
  bool _closed = false;
  RuntimeConnection? _connection;
  void Function(Object)? _directOnError;
  bool _connecting = false;
  Completer<void>? _started;
  Future<void>? _closing;
  final _work = StreamController<void>.broadcast();
  final _changes = StreamController<void>.broadcast();
  final _completions = StreamController<Map<String, dynamic>>.broadcast(
    sync: true,
  );
  Stream<Map<String, dynamic>> get actionCompletions => _completions.stream;
  late final ActionObservers _actionObservers = ActionObservers();
  void _deliverCompletions(Iterable<Map<String, dynamic>> completions) {
    final events = completions.toList();
    // Settle every internal waiter before invoking application stream listeners.
    for (final event in events) {
      _actionObservers.complete(event);
    }
    for (final event in events) {
      _completions.add(event);
    }
  }

  ActionError _publicActionError(Object error) {
    if (error is ActionError) return error;
    if (error is ActionTransportException) {
      return ActionError(
        error.code,
        execution: error.execution,
        cause: error.cause,
      );
    }
    final transactionActive =
        error is StateError && error.message == 'transaction_active';
    return ActionError(
      transactionActive ? 'transaction_active' : 'action.transport_failed',
      execution: transactionActive ? 'rejected' : 'unknown',
      cause: error,
    );
  }

  final Object _txZoneKey = Object();
  Object? _activeTxToken;
  Client._(this._worker, this._isolate, this._handle, this.clientId);
  static Future<Client> open({
    required String path,
    required Map<String, dynamic> schema,
    String? libraryPath,
    Map<String, dynamic>? migration,

    /// Rebuild at once when the schema is incompatible, leaving unsent work in the old file.
    bool discardPending = false,
  }) async {
    final ready = ReceivePort();
    final isolate = await Isolate.spawn(_nativeWorker, [
      ready.sendPort,
      libraryPath,
    ]);
    final response = await ready.first;
    ready.close();
    if (response is! SendPort) {
      isolate.kill();
      throw StateError('$response');
    }
    try {
      final opened = await _request(response, {
        'op': 'open',
        'path': path,
        'schema': schema,
        if (migration != null) 'migration': migration,
        if (discardPending) 'discardPending': true,
      });
      final value = opened['value'] as Map;
      return Client._(
        response,
        isolate,
        value['handle'] as int,
        value['clientId'] as String,
      );
    } catch (_) {
      response.send(null);
      isolate.kill();
      rethrow;
    }
  }

  static Future<Map<String, dynamic>> _request(
    SendPort worker,
    Map<String, dynamic> request,
  ) async {
    final reply = ReceivePort();
    worker.send([jsonEncode(request), reply.sendPort]);
    try {
      final response =
          jsonDecode(await reply.first as String) as Map<String, dynamic>;
      if (response['ok'] != true) throw StateError(response['error'] as String);
      return response['result'] as Map<String, dynamic>;
    } finally {
      reply.close();
    }
  }

  Future<T> _exclusive<T>(Future<T> Function() body) {
    final work = _tail.then((_) => body());
    _tail = work.then<void>((_) {}, onError: (Object _, StackTrace __) {});
    return work;
  }

  Future<dynamic> _send(Map<String, dynamic> request) async {
    if (_closed) throw StateError('client_closed');
    final response = await _request(_worker, {'handle': _handle, ...request});
    if (response['changed'] == true) _changes.add(null);
    return response['value'];
  }

  Future<T> transaction<T>(Future<T> Function(Transaction tx) body) =>
      _exclusive(() async {
        await _send({'op': 'begin'});
        final tx = Transaction._(this);
        try {
          final token = Object();
          _activeTxToken = token;
          late T result;
          try {
            result = await runZoned(
              () => body(tx),
              zoneValues: {_txZoneKey: token},
            );
          } finally {
            _activeTxToken = null;
          }
          await tx._finish();
          await _send({'op': 'commit'});
          _work.add(null);
          return result;
        } catch (error, stack) {
          try {
            await tx._finish();
          } catch (_) {}
          try {
            await _send({'op': 'rollback'});
          } catch (_) {}
          Error.throwWithStackTrace(error, stack);
        }
      });
  Future<Map<String, dynamic>?> read(
    String model,
    Map<String, dynamic> identity,
  ) => _exclusive(
    () async =>
        (await _send({
              'op': 'read',
              'key': {'model': model, 'identity': identity},
            }))
            as Map<String, dynamic>?,
  );
  Future<List<Map<String, dynamic>>> query(
    String model, {
    Map<String, dynamic> where = const {},
  }) => _exclusive(
    () async =>
        (await _send({'op': 'query', 'model': model, 'filter': where}) as List)
            .cast<Map<String, dynamic>>(),
  );
  Future<List<Map<String, dynamic>>> readSql(
    String sql, {
    List<dynamic> parameters = const [],
  }) => _exclusive(
    () async =>
        (await _send({'op': 'sql', 'sql': sql, 'parameters': parameters})
                as List)
            .cast<Map<String, dynamic>>(),
  );
  Future<List<Map<String, dynamic>>> querySpec(
    String model,
    Map<String, dynamic> query,
  ) => _exclusive(
    () async =>
        (await _send({'op': 'querySpec', 'model': model, 'query': query})
                as List)
            .cast<Map<String, dynamic>>(),
  );
  Future<Map<String, dynamic>?> related(
    String model,
    Map<String, dynamic> identity,
    String relation,
  ) => _exclusive(
    () async =>
        await _send({
              'op': 'related',
              'key': {'model': model, 'identity': identity},
              'relation': relation,
            })
            as Map<String, dynamic>?,
  );
  Future<List<Map<String, dynamic>>> referencing(
    String model,
    Map<String, dynamic> identity,
    String source,
    String relation,
  ) => _exclusive(
    () async =>
        (await _send({
                  'op': 'referencing',
                  'key': {'model': model, 'identity': identity},
                  'source': source,
                  'relation': relation,
                })
                as List)
            .cast<Map<String, dynamic>>(),
  );
  Future<int> mutate(Map<String, dynamic> mutation) {
    if (_activeTxToken != null &&
        identical(Zone.current[_txZoneKey], _activeTxToken)) {
      return Future.error(StateError('transaction_active'));
    }
    return _submitMutation(mutation);
  }

  /// One framework-owned local transaction; no backend work is queued.
  Future<void> direct(Map<String, dynamic> operation) {
    if (_activeTxToken != null &&
        identical(Zone.current[_txZoneKey], _activeTxToken)) {
      return Future.error(StateError('transaction_active'));
    }
    return transaction((tx) => tx.direct(operation));
  }

  Future<ActionCall<T>> invokeAction<T>(
    String name,
    int version,
    Map<String, dynamic> args,
    T Function(dynamic) decode, {
    ActionStore? store,
  }) async {
    ActionCall<T>? call;
    try {
      await submitAction(
        name,
        version,
        args,
        onCommitted: (callId, _) {
          call = _actionObservers.register(callId, decode);
        },
        store: store,
      );
    } catch (error) {
      throw _publicActionError(error);
    }
    return call!;
  }

  Future<T> invokeDirectAction<T>(
    String name,
    int version,
    Map<String, dynamic> args,
    T Function(dynamic) decode, {
    ActionStore? store,
  }) async {
    late final Map<String, dynamic> applied;
    try {
      applied = await callAction(name, version, args, store: store);
    } catch (error) {
      throw _publicActionError(error);
    }
    final completions = applied['completions'] as List?;
    if (completions == null || completions.isEmpty) {
      throw const ActionError('action.observation_failed');
    }
    final completion = completions.first as Map;
    final outcome = completion['outcome'] as Map;
    if (outcome['status'] != 'succeeded') {
      throw ActionError(
        outcome['code'] as String? ?? 'action.failed',
        execution: outcome['execution'] as String? ?? 'rejected',
      );
    }
    try {
      return decode(outcome['result']);
    } catch (error) {
      throw ActionError('action.observation_failed', cause: error);
    }
  }

  Future<int> _submitMutation(Map<String, dynamic> mutation) =>
      _exclusive(() async {
        await _send({'op': 'begin'});
        try {
          final ordinal =
              await _send({'op': 'enqueue', 'mutation': mutation}) as int;
          await _send({'op': 'commit'});
          _work.add(null);
          return ordinal;
        } catch (error, stack) {
          try {
            await _send({'op': 'rollback'});
          } catch (rollbackError) {
            throw _MutationRollbackError(error, rollbackError);
          }
          Error.throwWithStackTrace(error, stack);
        }
      });

  /// Internal Action seam: callback runs after local commit and before work wake.
  Future<Map<String, dynamic>> submitAction(
    String name,
    int version,
    Map<String, dynamic> args, {
    void Function(String callId, int ordinal)? onCommitted,
    ActionStore? store,
  }) {
    final wire = store?.toWire();
    if (_activeTxToken != null &&
        identical(Zone.current[_txZoneKey], _activeTxToken))
      return Future.error(StateError('transaction_active'));
    return _exclusive(() async {
      final submitted =
          (await _send({
                'op': 'submitAction',
                'name': name,
                'version': version,
                'args': args,
                if (wire != null) 'store': wire,
              }))
              as Map<String, dynamic>;
      onCommitted?.call(
        submitted['callId'] as String,
        submitted['ordinal'] as int,
      );
      _work.add(null);
      return submitted;
    });
  }

  Future<Map<String, dynamic>> callAction(
    String name,
    int version,
    Map<String, dynamic> args, {
    ActionStore? store,
  }) async {
    final wire = store?.toWire();
    if (_activeTxToken != null &&
        identical(Zone.current[_txZoneKey], _activeTxToken))
      throw StateError('transaction_active');
    final connection = _connection;
    if (connection == null || !connection.directAvailable || _closing != null)
      throw ActionTransportException('action.unavailable');
    final prepared =
        (await _exclusive(
              () async => await _send({
                'op': 'prepareAction',
                'name': name,
                'version': version,
                'args': args,
                if (wire != null) 'store': wire,
              }),
            ))
            as Map<String, dynamic>;
    final response = await connection.requestAction(prepared['body'] as String);
    if (!identical(_connection, connection) ||
        !connection.directAvailable ||
        _closing != null)
      throw ActionTransportException('action.execution_unknown');
    late final Map<String, dynamic> applied;
    try {
      applied =
          (await _exclusive(() async {
                if (!identical(_connection, connection) ||
                    !connection.directAvailable ||
                    _closing != null)
                  throw ActionTransportException('action.execution_unknown');
                return await _send({
                  'op': 'applyActionResponse',
                  'body': prepared['body'],
                  'response': jsonDecode(response),
                });
              }))
              as Map<String, dynamic>;
    } on ActionTransportException {
      rethrow;
    } catch (error) {
      throw ActionTransportException('action.execution_unknown', error);
    }
    _deliverCompletions(
      (applied['completions'] as List).cast<Map<String, dynamic>>(),
    );
    for (final report in applied['reports'] as List<dynamic>) {
      try {
        _directOnError?.call(
          AxtonReport.fromJson(report as Map<String, dynamic>),
        );
      } catch (error, stack) {
        Zone.current.handleUncaughtError(error, stack);
      }
    }
    return applied;
  }

  Future<void> subscribe(String channel) => _setChannel(channel, true);
  Future<void> unsubscribe(String channel) => _setChannel(channel, false);

  /// The live session is abandoned at once; once the change commits, Rust
  /// starts one for the new channel set.
  Future<void> _setChannel(String channel, bool subscribed) {
    _live?.cancel();
    return _exclusive(() async {
      try {
        await _send({
          'op': 'channel',
          'channel': channel,
          'subscribed': subscribed,
        });
      } catch (_) {
        _channels.add(null);
        rethrow;
      }
      _channels.add(null);
      _work.add(null);
    });
  }

  Future<RuntimeConnection> connect(
    SyncServer server, {
    void Function(Object)? onError,
    Future<void> Function()? refreshAuth,
    Duration directTimeout = const Duration(seconds: 30),
  }) async {
    final live = ServerSession(server);
    final transport = live.push;
    if (_closed || _closing != null) throw StateError('client_closed');
    if (_connecting || _connection != null)
      throw StateError('connection already active');
    _connecting = true;
    final started = Completer<void>();
    _started = started;
    try {
      Future<void>? refreshing;
      Future<void> refresh() =>
          refreshing ??= Future<void>.sync(refreshAuth!).whenComplete(() {
            refreshing = null;
          });
      final connection = await RuntimeConnection.start(
        control: (event, now, entropy) => _exclusive(
          () => _send({
            'op': 'connection',
            'event': event,
            'now': now,
            'entropy': entropy,
          }),
        ),
        sync: (transport) => _startSync(transport, true, onError),
        transport: transport,
        directCarrier: live.action,
        onError: onError,
        refreshAuth: refreshAuth == null ? null : refresh,
        directTimeout: directTimeout,
      );
      final streaming = await LiveLane.start(
        command: (event) => _exclusive(
          () async =>
              (await _send({
                    'op': 'live',
                    ...event,
                    'now': DateTime.now().millisecondsSinceEpoch,
                    'entropy': Random().nextInt(0x100000000),
                  }))
                  as List<dynamic>,
        ),
        network: live,
        wakePush: () => unawaited(
          connection.wake().catchError((Object error) {
            onError?.call(error);
          }),
        ),
        onError: onError,
        refreshAuth: refreshAuth == null ? null : refresh,
      );
      _live = streaming;
      final channelSubscription = _channels.stream.listen((_) {
        unawaited(
          streaming.wake().catchError((Object error) {
            onError?.call(error);
          }),
        );
      });
      connection.attachLive(streaming, live.cancelPush);
      final subscription = _work.stream.listen((_) {
        unawaited(
          connection.wake().catchError((Object error) {
            onError?.call(error);
          }),
        );
      });
      _connection = connection;
      _directOnError = onError;
      unawaited(
        connection.closed.then((_) async {
          await subscription.cancel();
          await channelSubscription.cancel();
          if (identical(_connection, connection)) {
            _connection = null;
            _live = null;
            _directOnError = null;
          }
        }),
      );
      return connection;
    } finally {
      _connecting = false;
      started.complete();
    }
  }

  Future<void> _startSync(
    Transport transport,
    bool pushOnly,
    void Function(Object)? onError,
  ) => _syncing ??= _runSync(transport, pushOnly, onError).whenComplete(() {
    _syncing = null;
  });
  Future<void> _runSync(
    Future<String> Function(String kind, String body) transport,
    bool pushOnly,
    void Function(Object)? onError,
  ) async {
    await _exclusive(() => _send({'op': 'startSync', 'pushOnly': pushOnly}));
    while (true) {
      final action = await _exclusive(() => _send({'op': 'next'}));
      if (action == null) return;
      final response = await transport(
        action['kind'] as String,
        action['body'] as String,
      );
      final applied =
          await _exclusive(
                () =>
                    _send({'op': 'complete', 'response': jsonDecode(response)}),
              )
              as Map<String, dynamic>;
      _deliverCompletions(
        (applied['completions'] as List).cast<Map<String, dynamic>>(),
      );
      // What the receipt or page could not apply; the client stays consistent
      // and the application hears about each one.
      for (final report in applied['reports'] as List<dynamic>) {
        try {
          onError?.call(AxtonReport.fromJson(report as Map<String, dynamic>));
        } catch (error, stack) {
          Zone.current.handleUncaughtError(error, stack);
        }
      }
    }
  }

  Future<void> runPrerequisites(
    Map<String, Future<void> Function(Map<String, dynamic>)> handlers,
  ) => _tasks ??= _runPrerequisites(handlers).whenComplete(() {
    _tasks = null;
  });
  // Rust picks the task and settles it; this loop only calls the handler.
  Future<void> _runPrerequisites(
    Map<String, Future<void> Function(Map<String, dynamic>)> handlers,
  ) async {
    final names = handlers.keys.toList();
    while (true) {
      final task = await _exclusive(
        () async =>
            await _send({'op': 'task', 'handlers': names})
                as Map<String, dynamic>?,
      );
      if (task == null) return;
      String? error;
      try {
        await handlers[task['name']]!(
          task['arguments'] as Map<String, dynamic>,
        );
      } catch (thrown) {
        error = _reason(thrown);
      }
      await _exclusive(() async {
        await _send({'op': 'outcome', 'key': task['key'], 'error': error});
      });
      _work.add(null);
    }
  }

  /// The text a failed prerequisite keeps.
  static String _reason(Object thrown) => thrown.toString();

  Future<String?> freeze() =>
      _exclusive(() async => await _send({'op': 'freeze'}) as String?);
  Future<void> acknowledge(int sequence, Map<String, dynamic> receipt) =>
      _exclusive(() async {
        final applied =
            (await _send({
                  'op': 'ack',
                  'sequence': sequence,
                  'receipt': receipt,
                }))
                as Map<String, dynamic>;
        _deliverCompletions(
          (applied['completions'] as List).cast<Map<String, dynamic>>(),
        );
      });
  Future<Map<String, dynamic>> applyPull(Map<String, dynamic> page) =>
      _exclusive(
        () async =>
            (await _send({'op': 'pull', 'page': page})) as Map<String, dynamic>,
      );

  /// One record's sync state: its pending mutations and retained rejections.
  Future<Map<String, dynamic>> recordSyncState(
    String model,
    Map<String, dynamic> identity,
  ) => _exclusive(
    () async =>
        await _send({
              'op': 'recordStatus',
              'key': {'model': model, 'identity': identity},
            })
            as Map<String, dynamic>,
  );

  /// The client's sync state: a local snapshot, not a network probe.
  Future<Map<String, dynamic>> syncState() => _exclusive(
    () async => (await _send({'op': 'status'})) as Map<String, dynamic>,
  );

  /// Leave an incompatible database behind and open a fresh file for the
  /// schema this client asked for. Refused while unsent mutations remain
  /// unless [discardPending]; the report says what the old file keeps.
  Future<Map<String, dynamic>> rebuild({bool discardPending = false}) =>
      _exclusive(() async {
        final report =
            (await _send({'op': 'rebuild', 'discardPending': discardPending}))
                as Map<String, dynamic>;
        _deliverCompletions(
          (report['abandonedCalls'] as List).map((abandoned) {
            final call = abandoned as Map<String, dynamic>;
            return {
              'callId': call['callId'],
              'outcome': {
                'status': 'failed',
                'code': 'abandoned',
                'execution': call['frozen'] == true ? 'unknown' : 'rejected',
              },
            };
          }),
        );
        return report;
      });
  Future<List<Map<String, dynamic>>> pendingTasks() => _exclusive(
    () async =>
        (await _send({'op': 'tasks'}) as List).cast<Map<String, dynamic>>(),
  );
  Future<void> setReadiness(String key, String state) => _exclusive(() async {
    await _send({'op': 'readiness', 'key': key, 'state': state});
    _work.add(null);
  });
  Future<void> drop(int ordinal) => _exclusive(() async {
    final result =
        (await _send({'op': 'drop', 'ordinal': ordinal}))
            as Map<String, dynamic>;
    _deliverCompletions(
      (result['completions'] as List).cast<Map<String, dynamic>>(),
    );
    _work.add(null);
  });
  Future<void> dismissRejection(int ordinal) => _exclusive(() async {
    await _send({'op': 'dismiss', 'ordinal': ordinal});
  });
  Stream<List<Map<String, dynamic>>> watch(
    String model, {
    Map<String, dynamic> where = const {},
  }) {
    return Stream<List<Map<String, dynamic>>>.multi((sink) {
      String? previous;
      bool cancelled = false;
      Future<void> pending = Future<void>.value();
      void refresh() {
        pending = pending.then((_) async {
          if (cancelled) return;
          try {
            final rows = await query(model, where: where);
            final value = jsonEncode(rows);
            if (!cancelled && value != previous) {
              previous = value;
              sink.add(rows);
            }
          } catch (e, st) {
            if (!cancelled) sink.addError(e, st);
          }
        });
      }

      final sub = _changes.stream.listen((_) => refresh(), onDone: sink.close);
      refresh();
      sink.onCancel = () async {
        cancelled = true;
        await sub.cancel();
      };
    });
  }

  Future<void> close() => _closing ??= _finishClose();

  Future<void> _finishClose() async {
    _actionObservers.close();
    await _started?.future;
    await _connection?.close();
    await _exclusive(() async {
      if (_closed) return;
      try {
        await _send({'op': 'close'});
      } finally {
        _closed = true;
        _worker.send(null);
        _isolate.kill();
        await _changes.close();
        await _completions.close();
        await _work.close();
        await _channels.close();
      }
    });
  }
}

class Transaction implements WritePort {
  final Client _client;
  bool _open = true;
  Transaction._(this._client);
  Future<void> _tail = Future<void>.value();
  int _pending = 0;
  Object? _failure;
  Object? _structural;
  final Object _zoneKey = Object();
  Object? _active;
  final Set<Future<dynamic>> _scopes = {};
  Future<dynamic> _queue(Map<String, dynamic> request) {
    _pending++;
    final work = _tail.then(
      (_) => _client._send({...request, 'transaction': true}),
    );
    _tail = work.then<void>(
      (_) {
        _pending--;
      },
      onError: (Object error, StackTrace stack) {
        _pending--;
        _failure ??= error;
      },
    );
    return work;
  }

  Future<dynamic> _send(Map<String, dynamic> request) {
    if (!_open) return Future.error(StateError('transaction_closed'));
    if (_active != null && Zone.current[_zoneKey] != _active) {
      _structural = StateError('overlapping savepoint work');
      return Future.error(_structural!);
    }
    return _queue(request);
  }

  Future<void> _finish() async {
    final outstanding = _pending > 0 || _scopes.isNotEmpty;
    _open = false;
    await _tail;
    if (_structural != null) throw _structural!;
    if (outstanding) throw StateError('unawaited transaction operation');
    if (_failure != null) throw _failure!;
  }

  Future<Map<String, dynamic>?> read(
    String model,
    Map<String, dynamic> identity,
  ) async =>
      (await _send({
            'op': 'read',
            'key': {'model': model, 'identity': identity},
          }))
          as Map<String, dynamic>?;
  Future<List<Map<String, dynamic>>> query(
    String model, {
    Map<String, dynamic> where = const {},
  }) async =>
      (await _send({'op': 'query', 'model': model, 'filter': where}) as List)
          .cast<Map<String, dynamic>>();
  Future<List<Map<String, dynamic>>> readSql(
    String sql, {
    List<dynamic> parameters = const [],
  }) async =>
      (await _send({'op': 'sql', 'sql': sql, 'parameters': parameters}) as List)
          .cast<Map<String, dynamic>>();
  Future<List<Map<String, dynamic>>> querySpec(
    String model,
    Map<String, dynamic> query,
  ) async =>
      (await _send({'op': 'querySpec', 'model': model, 'query': query}) as List)
          .cast<Map<String, dynamic>>();
  Future<Map<String, dynamic>?> related(
    String model,
    Map<String, dynamic> identity,
    String relation,
  ) async =>
      await _send({
            'op': 'related',
            'key': {'model': model, 'identity': identity},
            'relation': relation,
          })
          as Map<String, dynamic>?;
  Future<List<Map<String, dynamic>>> referencing(
    String model,
    Map<String, dynamic> identity,
    String source,
    String relation,
  ) async =>
      (await _send({
                'op': 'referencing',
                'key': {'model': model, 'identity': identity},
                'source': source,
                'relation': relation,
              })
              as List)
          .cast<Map<String, dynamic>>();
  Future<void> direct(Map<String, dynamic> operation) async {
    await _send({'op': 'direct', 'operation': operation});
  }

  Future<T> savepoint<T>(Future<T> Function() body) {
    if (!_open) return Future.error(StateError('transaction_closed'));
    if (_active != null && Zone.current[_zoneKey] != _active) {
      _structural = StateError('overlapping savepoints');
      return Future.error(_structural!);
    }
    final parent = _active;
    final token = Object();
    _active = token;
    final failure = _failure;
    final run = runZoned(() async {
      await _queue({'op': 'savepoint'});
      try {
        if (!_open) throw StateError('transaction_closed');
        final result = await body();
        await _tail;
        if (!_open) throw StateError('transaction_closed');
        if (_active != token) {
          _structural = StateError('unawaited nested savepoint');
          throw _structural!;
        }
        if (_failure != failure) throw _failure!;
        if (_structural != null) throw _structural!;
        await _queue({'op': 'release'});
        return result;
      } catch (error, stack) {
        await _tail;
        if (_open && _structural == null) {
          if (_active != token) {
            _structural = StateError('unawaited nested savepoint');
            throw _structural!;
          }
          await _queue({'op': 'rollbackSavepoint'});
          _failure = failure;
        }
        Error.throwWithStackTrace(error, stack);
      } finally {
        if (_active == token) _active = parent;
      }
    }, zoneValues: {_zoneKey: token});
    _scopes.add(run);
    unawaited(
      run.then<void>(
        (_) {
          _scopes.remove(run);
        },
        onError: (Object _, StackTrace __) {
          _scopes.remove(run);
        },
      ),
    );
    return run;
  }
}
