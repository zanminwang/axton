import 'actions.dart';
import 'bridge.dart';
import 'connection.dart';
import 'live.dart';
import 'port.dart';
import 'subscriptions.dart';
import 'dart:async';
import 'dart:convert';
import 'dart:math';

/// Typed generated model APIs delegate to this generic native client.
class Client implements WritePort, MutatePort {
  /// The Rust-owned runtime: it orders every task and owns the database.
  final Bridge _bridge;
  final String clientId;
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

  CallError _publicActionError(Object error) {
    if (error is CallError) return error;
    if (error is ActionTransportException) {
      return CallError(
        error.code,
        execution: error.execution,
        cause: error.cause,
      );
    }
    final transactionActive =
        error is StateError && error.message == 'transaction_active';
    return CallError(
      transactionActive ? 'transaction_active' : 'action.transport_failed',
      execution: transactionActive ? 'rejected' : 'unknown',
      cause: error,
    );
  }

  /// Subscription handles by persistent identity, and the status they publish.
  late final Subscriptions _subscriptions = Subscriptions(
    SubscriptionCommands(
      subscribe: (scope) async => SubscriptionState.fromRecord(
        (await _bridge.task({'kind': 'scopeSubscribe', 'scope': scope}))
            as Map<String, dynamic>,
      ),
      state: (scope) async {
        final state = await _bridge.task({
          'kind': 'scopeState',
          'scope': scope,
        });
        return state == null
            ? null
            : SubscriptionState.fromRecord(state as Map<String, dynamic>);
      },
      remove: (scope, subscriptionId) async =>
          ((await _bridge.task({
                'kind': 'scopeUnsubscribe',
                'scope': scope,
                'subscriptionId': subscriptionId,
              }))
              as Map<String, dynamic>)['removed'] ==
          true,
      removeScope: (scope) async {
        await _bridge.task({
          'kind': 'channel',
          'channel': scope,
          'subscribed': false,
        });
      },
      // Registration and the state read of one identity's durable load: local
      // tasks Rust runs in submission order, so calls for one Scope keep their
      // order ([#151](https://github.com/zanminwang/axton/issues/151)).
      requestBootstrap: (scope, subscriptionId) async =>
          BootstrapRun.fromRecord(
            (await _bridge.task({
                  'kind': 'scopeBootstrap',
                  'scope': scope,
                  'subscriptionId': subscriptionId,
                }))
                as Map<String, dynamic>,
          ),
      bootstrapState: (scope, subscriptionId) async => BootstrapRun.fromRecord(
        (await _bridge.task({
              'kind': 'scopeBootstrapState',
              'scope': scope,
              'subscriptionId': subscriptionId,
            }))
            as Map<String, dynamic>,
      ),
      // A committed membership change wakes the lanes; Rust decides what it
      // means for the socket.
      committed: () {
        _channels.add(null);
        _work.add(null);
      },
    ),
  );

  /// The Scope surface the generated `scopes` facade delegates to, with no
  /// logic of its own.
  late final ClientScopes scopes = ClientScopes(this);

  final Object _txZoneKey = Object();
  Object? _activeTxToken;
  Client._(this._bridge, this.clientId) {
    _bridge.changed.listen((_) {
      if (!_changes.isClosed) _changes.add(null);
    });
  }
  static Future<Client> open({
    required String path,
    required Map<String, dynamic> schema,
    String? libraryPath,
    Map<String, dynamic>? migration,

    /// Rebuild at once when the schema is incompatible, leaving unsent work in the old file.
    bool discardPending = false,
  }) async {
    final bridge = await Bridge.open(
      path: path,
      schema: schema,
      libraryPath: libraryPath,
      migration: migration,
      discardPending: discardPending,
    );
    return Client._(bridge, bridge.opened['clientId'] as String);
  }

  /// Rust runs [body] as the callback of a local transaction it owns: ordinary
  /// reads and writes wait until it commits or rolls back, and the result is
  /// returned only once the commit is confirmed.
  Future<T> transaction<T>(Future<T> Function(Transaction tx) body) async {
    late T result;
    await _bridge.transaction((transactionId) async {
      final tx = Transaction._(this, transactionId);
      try {
        final token = Object();
        _activeTxToken = token;
        try {
          result = await runZoned(
            () => body(tx),
            zoneValues: {_txZoneKey: token},
          );
        } finally {
          _activeTxToken = null;
        }
      } catch (error, stack) {
        try {
          await tx._finish();
        } catch (_) {}
        Error.throwWithStackTrace(error, stack);
      }
      await tx._finish();
    });
    _work.add(null);
    return result;
  }

  Future<Map<String, dynamic>?> read(
    String model,
    Map<String, dynamic> identity,
  ) async =>
      (await _bridge.task({
            'kind': 'read',
            'key': {'model': model, 'identity': identity},
          }))
          as Map<String, dynamic>?;
  Future<List<Map<String, dynamic>>> query(
    String model, {
    Map<String, dynamic> where = const {},
  }) async =>
      (await _bridge.task({'kind': 'query', 'model': model, 'filter': where})
              as List)
          .cast<Map<String, dynamic>>();
  Future<List<Map<String, dynamic>>> readSql(
    String sql, {
    List<dynamic> parameters = const [],
  }) async =>
      (await _bridge.task({'kind': 'sql', 'sql': sql, 'parameters': parameters})
              as List)
          .cast<Map<String, dynamic>>();
  Future<List<Map<String, dynamic>>> querySpec(
    String model,
    Map<String, dynamic> query,
  ) async =>
      (await _bridge.task({'kind': 'querySpec', 'model': model, 'query': query})
              as List)
          .cast<Map<String, dynamic>>();
  Future<Map<String, dynamic>?> related(
    String model,
    Map<String, dynamic> identity,
    String relation,
  ) async =>
      await _bridge.task({
            'kind': 'related',
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
      (await _bridge.task({
                'kind': 'referencing',
                'key': {'model': model, 'identity': identity},
                'source': source,
                'relation': relation,
              })
              as List)
          .cast<Map<String, dynamic>>();
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

  Future<Call<T>> invokeAction<T>(
    String name,
    int version,
    Map<String, dynamic> args,
    T Function(dynamic) decode, {
    CallStore? store,
  }) async {
    Call<T>? call;
    final closedBefore = _closed;
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
      // Close is priority control: a submission still queued when the client
      // began closing never runs. Its caller gets the handle close gives every
      // call it can no longer observe, as when the submission ran first.
      if (!closedBefore &&
          _closing != null &&
          error is StateError &&
          error.message == 'client_closed') {
        return _actionObservers.register<T>('', decode);
      }
      throw _publicActionError(error);
    }
    return call!;
  }

  Future<T> invokeDirectAction<T>(
    String name,
    int version,
    Map<String, dynamic> args,
    T Function(dynamic) decode, {
    CallStore? store,
  }) async {
    late final Map<String, dynamic> applied;
    try {
      applied = await callAction(name, version, args, store: store);
    } catch (error) {
      throw _publicActionError(error);
    }
    final completions = applied['completions'] as List?;
    if (completions == null || completions.isEmpty) {
      throw const CallError('action.observation_failed');
    }
    final completion = completions.first as Map;
    final outcome = completion['outcome'] as Map;
    if (outcome['status'] != 'succeeded') {
      throw CallError(
        outcome['code'] as String? ?? 'action.failed',
        execution: outcome['execution'] as String? ?? 'rejected',
      );
    }
    try {
      return decode(outcome['result']);
    } catch (error) {
      throw CallError('action.observation_failed', cause: error);
    }
  }

  /// Execute a direct Query. Without [once] it is exactly
  /// [invokeDirectAction]: a fresh request that reads and writes no
  /// snapshot. With [once], Rust decides: a saved result is decoded without
  /// any request or Model write, an active request is joined, or a new one
  /// is executed and its successful result saved with its authority.
  /// [refresh] (only with [once]) always requests and replaces on success.
  Future<T> invokeQuery<T>(
    String name,
    int version,
    Map<String, dynamic> args,
    T Function(dynamic) decode, {
    CallStore? store,
    bool once = false,
    bool refresh = false,
  }) async {
    if (refresh && !once) {
      throw CallError(
        'action.invalid_options',
        execution: 'rejected',
        cause: ArgumentError('refresh requires once: true'),
      );
    }
    if (!once) {
      return invokeDirectAction<T>(name, version, args, decode, store: store);
    }
    late final String outcome;
    try {
      outcome = await _queryOnce(name, version, args, refresh, store);
    } catch (error) {
      throw _publicActionError(error);
    }
    return _decodeOutcome(jsonDecode(outcome) as Map, decode);
  }

  /// Discard the saved once results of one Query argument set, every store
  /// variant, in a local transaction. Needs no network; an older request
  /// still in flight cannot save its result afterwards.
  Future<void> invalidateQuery(
    String name,
    int version,
    Map<String, dynamic> args,
  ) async {
    try {
      if (_activeTxToken != null &&
          identical(Zone.current[_txZoneKey], _activeTxToken)) {
        throw StateError('transaction_active');
      }
      await _bridge.task({
        'kind': 'invalidateQueryOnce',
        'name': name,
        'version': version,
        'args': args,
      });
    } catch (error) {
      throw _publicActionError(error);
    }
  }

  T _decodeOutcome<T>(Map outcome, T Function(dynamic) decode) {
    if (outcome['status'] != 'succeeded') {
      throw CallError(
        outcome['code'] as String? ?? 'action.failed',
        execution: outcome['execution'] as String? ?? 'rejected',
      );
    }
    try {
      return decode(outcome['result']);
    } catch (error) {
      throw CallError('action.observation_failed', cause: error);
    }
  }

  /// Settles once every task submitted before it has run: Rust runs ordinary
  /// tasks in submission order and holds them all while a local transaction
  /// callback owns the writer. A direct response applies only if its
  /// connection is still current at its turn, which Rust cannot judge until
  /// the direct call itself moves into the runtime (checkpoint 2 of #134).
  Future<void> _turn() =>
      _bridge.task(const {'kind': 'sql', 'sql': 'SELECT 1', 'parameters': []});

  /// Active once flights by Rust flight ID; removed at their terminal step.
  /// Each holds only the raw outcome text; every caller decodes its own copy.
  final _queryFlights = <String, Completer<String>>{};

  Future<String> _queryOnce(
    String name,
    int version,
    Map<String, dynamic> args,
    bool refresh,
    CallStore? store,
  ) async {
    final wire = store?.toWire();
    if (_activeTxToken != null &&
        identical(Zone.current[_txZoneKey], _activeTxToken)) {
      throw StateError('transaction_active');
    }
    // Rust decides in task order, and the bridge dispatches completions in
    // that order, so a `join` for flight F settles only after the `fetch` that
    // opened F. Every decision passes through this same single await before
    // touching `_queryFlights`, so the fetching caller registers F in an
    // earlier microtask than any caller that joins it looks it up.
    // Checkpoint 3 of #134 moves the flight into Rust.
    final decided =
        (await _bridge.task({
              'kind': 'queryOnce',
              'name': name,
              'version': version,
              'args': args,
              'refresh': refresh,
              if (wire != null) 'store': wire,
            }))
            as Map<String, dynamic>;
    final flightId = decided['flightId'] as String?;
    switch (decided['decision']) {
      case 'cached':
        return jsonEncode({'status': 'succeeded', 'result': decided['result']});
      case 'join':
        final joined = _queryFlights[flightId];
        if (joined == null) {
          throw ActionTransportException('action.execution_unknown');
        }
        return joined.future;
    }
    final completer = Completer<String>();
    // Settled callers observe it; an unobserved failure is not an error.
    completer.future.ignore();
    _queryFlights[flightId!] = completer;
    final current = _connection;
    if (current == null || !current.directAvailable || _closing != null) {
      // Registered first so a caller Rust already joined to it hears the same.
      await _releaseQueryFlight(
        flightId,
        completer,
        ActionTransportException('action.unavailable'),
      );
      return completer.future;
    }
    unawaited(
      _runQueryFlight(flightId, decided['body'] as String, current, completer),
    );
    return completer.future;
  }

  /// Tell Rust the flight ended without a result, then fail its callers. The
  /// flight stays registered until Rust retired it, so a caller Rust joined to
  /// it before then still finds it.
  Future<void> _releaseQueryFlight(
    String flightId,
    Completer<String> flight,
    Object error,
  ) async {
    if (identical(_queryFlights[flightId], flight)) {
      try {
        await _bridge.task({'kind': 'failQueryOnce', 'flightId': flightId});
      } catch (_) {}
      if (identical(_queryFlights[flightId], flight)) {
        _queryFlights.remove(flightId);
      }
    }
    if (!flight.isCompleted) flight.completeError(error);
  }

  /// Execute one fetched flight and settle every caller joined to it.
  Future<void> _runQueryFlight(
    String flightId,
    String body,
    RuntimeConnection connection,
    Completer<String> flight,
  ) async {
    void fail(Object error) {
      if (!flight.isCompleted) flight.completeError(error);
    }

    late final String response;
    try {
      response = await connection.requestAction(body);
    } on ActionTransportException catch (error) {
      return _releaseQueryFlight(flightId, flight, error);
    } catch (error) {
      return _releaseQueryFlight(
        flightId,
        flight,
        ActionTransportException('action.execution_unknown', error),
      );
    }
    late final Map<String, dynamic> applied;
    try {
      final parsed = jsonDecode(response);
      await _turn();
      if (!identical(_connection, connection) ||
          !connection.directAvailable ||
          _closing != null) {
        throw ActionTransportException('action.execution_unknown');
      }
      applied =
          (await _bridge
                  .task({
                    'kind': 'finishQueryOnce',
                    'flightId': flightId,
                    'response': parsed,
                  })
                  .whenComplete(() {
                    // Rust retires the flight on every outcome of this task.
                    if (identical(_queryFlights[flightId], flight)) {
                      _queryFlights.remove(flightId);
                    }
                  }))
              as Map<String, dynamic>;
    } catch (error) {
      final mapped = error is ActionTransportException
          ? error
          : ActionTransportException('action.execution_unknown', error);
      if (identical(_queryFlights[flightId], flight)) {
        return _releaseQueryFlight(flightId, flight, mapped);
      }
      return fail(mapped);
    }
    final completions = (applied['completions'] as List)
        .cast<Map<String, dynamic>>();
    _deliverCompletions(completions);
    for (final report in applied['reports'] as List<dynamic>) {
      try {
        _directOnError?.call(
          AxtonReport.fromJson(report as Map<String, dynamic>),
        );
      } catch (error, stack) {
        Zone.current.handleUncaughtError(error, stack);
      }
    }
    if (completions.isEmpty) {
      return fail(const CallError('action.observation_failed'));
    }
    if (!flight.isCompleted) {
      flight.complete(jsonEncode(completions.first['outcome']));
    }
  }

  /// One task: Rust enqueues the mutation in its own local transaction.
  Future<int> _submitMutation(Map<String, dynamic> mutation) async {
    final ordinal =
        await _bridge.task({'kind': 'enqueue', 'mutation': mutation}) as int;
    _work.add(null);
    return ordinal;
  }

  /// Internal Action seam: callback runs after local commit and before work wake.
  Future<Map<String, dynamic>> submitAction(
    String name,
    int version,
    Map<String, dynamic> args, {
    void Function(String callId, int ordinal)? onCommitted,
    CallStore? store,
  }) {
    final wire = store?.toWire();
    if (_activeTxToken != null &&
        identical(Zone.current[_txZoneKey], _activeTxToken))
      return Future.error(StateError('transaction_active'));
    return _bridge
        .task({
          'kind': 'submitAction',
          'name': name,
          'version': version,
          'args': args,
          if (wire != null) 'store': wire,
        })
        .then((value) {
          final submitted = value as Map<String, dynamic>;
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
    CallStore? store,
  }) async {
    final wire = store?.toWire();
    if (_activeTxToken != null &&
        identical(Zone.current[_txZoneKey], _activeTxToken))
      throw StateError('transaction_active');
    final connection = _connection;
    if (connection == null || !connection.directAvailable || _closing != null)
      throw ActionTransportException('action.unavailable');
    final prepared =
        (await _bridge.task({
              'kind': 'prepareAction',
              'name': name,
              'version': version,
              'args': args,
              if (wire != null) 'store': wire,
            }))
            as Map<String, dynamic>;
    final response = await connection.requestAction(prepared['body'] as String);
    if (!identical(_connection, connection) ||
        !connection.directAvailable ||
        _closing != null)
      throw ActionTransportException('action.execution_unknown');
    late final Map<String, dynamic> applied;
    try {
      await _turn();
      if (!identical(_connection, connection) ||
          !connection.directAvailable ||
          _closing != null)
        throw ActionTransportException('action.execution_unknown');
      applied =
          (await _bridge.task({
                'kind': 'applyActionResponse',
                'body': prepared['body'],
                'response': jsonDecode(response),
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

  /// Register durable intent to follow [scope] and answer with its handle. It
  /// resolves when the local transaction commits: it awaits no
  /// authentication, connection or acknowledgement, and the same Scope answers
  /// with the same handle while its registration lives. The socket is never
  /// cancelled here; the Downlink worker sees the committed change and
  /// reconciles its own session.
  Future<Subscription> subscribeScope(String scope) =>
      _subscriptions.subscribe(scope);
  Future<Subscription> subscribe(String channel) => subscribeScope(channel);

  /// Remove whatever registration this Scope name has; its handle stops.
  Future<void> unsubscribe(String channel) =>
      _subscriptions.unsubscribeScope(channel);

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
        control: (event, now, entropy) => _bridge.task({
          'kind': 'connection',
          'event': event,
          'now': now,
          'entropy': entropy,
        }),
        sync: (transport) => _startSync(transport, true, onError),
        transport: transport,
        directCarrier: live.action,
        onError: onError,
        refreshAuth: refreshAuth == null ? null : refresh,
        directTimeout: directTimeout,
      );
      final streaming = await DownlinkLane.start(
        command: (event) async =>
            (await _bridge.task({
                  'kind': 'downlink',
                  ...event,
                  'now': DateTime.now().millisecondsSinceEpoch,
                  'entropy': Random().nextInt(0x100000000),
                }))
                as List<dynamic>,
        network: live,
        wakePush: () => unawaited(
          connection.wake().catchError((Object error) {
            onError?.call(error);
          }),
        ),
        onError: onError,
        refreshAuth: refreshAuth == null ? null : refresh,
        report: _subscriptions.signal,
      );
      _subscriptions.attach();
      final channelSubscription = _channels.stream.listen((_) {
        unawaited(
          streaming.wake().catchError((Object error) {
            onError?.call(error);
          }),
        );
      });
      connection.attachDownlink(streaming, live.cancelPush);
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
          _subscriptions.detach();
          if (identical(_connection, connection)) {
            _connection = null;
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
    await _bridge.task({'kind': 'startSync', 'pushOnly': pushOnly});
    while (true) {
      final action = await _bridge.task({'kind': 'next'});
      if (action == null) return;
      final response = await transport(
        action['kind'] as String,
        action['body'] as String,
      );
      final applied =
          await _bridge.task({
                'kind': 'complete',
                'response': jsonDecode(response),
              })
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
      final task =
          await _bridge.task({'kind': 'task', 'handlers': names})
              as Map<String, dynamic>?;
      if (task == null) return;
      String? error;
      try {
        await handlers[task['name']]!(
          task['arguments'] as Map<String, dynamic>,
        );
      } catch (thrown) {
        error = _reason(thrown);
      }
      await _bridge.task({
        'kind': 'outcome',
        'key': task['key'],
        'error': error,
      });
      _work.add(null);
    }
  }

  /// The text a failed prerequisite keeps.
  static String _reason(Object thrown) => thrown.toString();

  Future<String?> freeze() async =>
      await _bridge.task({'kind': 'freeze'}) as String?;
  Future<void> acknowledge(int sequence, Map<String, dynamic> receipt) async {
    final applied =
        (await _bridge.task({
              'kind': 'ack',
              'sequence': sequence,
              'receipt': receipt,
            }))
            as Map<String, dynamic>;
    _deliverCompletions(
      (applied['completions'] as List).cast<Map<String, dynamic>>(),
    );
  }

  Future<Map<String, dynamic>> applyPull(Map<String, dynamic> page) async =>
      (await _bridge.task({'kind': 'pull', 'page': page}))
          as Map<String, dynamic>;

  /// One record's sync state: its pending mutations and retained rejections.
  Future<Map<String, dynamic>> recordSyncState(
    String model,
    Map<String, dynamic> identity,
  ) async =>
      await _bridge.task({
            'kind': 'recordStatus',
            'key': {'model': model, 'identity': identity},
          })
          as Map<String, dynamic>;

  /// The client's sync state: a local snapshot, not a network probe.
  Future<Map<String, dynamic>> syncState() async =>
      (await _bridge.task({'kind': 'status'})) as Map<String, dynamic>;

  /// Leave an incompatible database behind and open a fresh file for the
  /// schema this client asked for. Refused while unsent mutations remain
  /// unless [discardPending]; the report says what the old file keeps.
  Future<Map<String, dynamic>> rebuild({bool discardPending = false}) async {
    final report =
        (await _bridge.task({
              'kind': 'rebuild',
              'discardPending': discardPending,
            }))
            as Map<String, dynamic>;
    // The replica that answered every handle is gone: no handle from before
    // it names a registration of the file this client now reads.
    _subscriptions.rebuilt();
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
  }

  Future<List<Map<String, dynamic>>> pendingTasks() async =>
      (await _bridge.task({'kind': 'tasks'}) as List)
          .cast<Map<String, dynamic>>();
  Future<void> setReadiness(String key, String state) async {
    await _bridge.task({'kind': 'readiness', 'key': key, 'state': state});
    _work.add(null);
  }

  Future<void> drop(int ordinal) async {
    final result =
        (await _bridge.task({'kind': 'drop', 'ordinal': ordinal}))
            as Map<String, dynamic>;
    _deliverCompletions(
      (result['completions'] as List).cast<Map<String, dynamic>>(),
    );
    _work.add(null);
  }

  Future<void> dismissRejection(int ordinal) async {
    await _bridge.task({'kind': 'dismiss', 'ordinal': ordinal});
  }

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
    for (final flight in _queryFlights.values) {
      if (!flight.isCompleted) {
        flight.completeError(const CallError('client.closed'));
      }
    }
    _queryFlights.clear();
    _subscriptions.close();
    await _started?.future;
    await _connection?.close();
    try {
      await _bridge.close();
    } finally {
      _closed = true;
      await _changes.close();
      await _completions.close();
      await _work.close();
      await _channels.close();
    }
  }
}

/// The Scope surface of one client: what the generated `scopes` facade
/// delegates to.
class ClientScopes {
  final Client _client;
  const ClientScopes(this._client);
  Future<Subscription> subscribe(String scope) => _client.subscribeScope(scope);
}

/// The application callback's handle on the local transaction Rust owns.
/// Its commands carry the runtime's transaction id and the savepoint scope of
/// the zone they are issued from; Rust runs them in submission order.
class Transaction implements WritePort {
  final Client _client;
  final String _transactionId;
  bool _open = true;
  Transaction._(this._client, this._transactionId);

  /// Settles once every command submitted so far has settled.
  Future<void> _tail = Future<void>.value();
  int _pending = 0;
  Object? _failure;
  Object? _structural;
  final Object _zoneKey = Object();
  Object? _active;

  /// The scope Rust answered for each open savepoint's zone token.
  final Map<Object, String> _scopeTokens = {};
  final Set<Future<dynamic>> _scopes = {};

  /// The savepoint scope of [zone]: that of its innermost savepoint, or null
  /// at the top level.
  String? _scopeOf(Zone zone) {
    final token = zone[_zoneKey];
    return token == null ? null : _scopeTokens[token];
  }

  Future<dynamic> _queue(Map<String, dynamic> command, String? scope) {
    _pending++;
    final work = _client._bridge.transactionCommand(
      _transactionId,
      scope,
      command,
    );
    final settled = work.then<void>(
      (_) {
        _pending--;
      },
      onError: (Object error, StackTrace stack) {
        _pending--;
        _failure ??= error;
      },
    );
    _tail = _tail.then((_) => settled);
    return work;
  }

  Future<dynamic> _send(Map<String, dynamic> command) {
    if (!_open) return Future.error(StateError('transaction_closed'));
    if (_active != null && Zone.current[_zoneKey] != _active) {
      _structural = StateError('overlapping savepoint work');
      return Future.error(_structural!);
    }
    return _queue(command, _scopeOf(Zone.current));
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
            'kind': 'read',
            'key': {'model': model, 'identity': identity},
          }))
          as Map<String, dynamic>?;
  Future<List<Map<String, dynamic>>> query(
    String model, {
    Map<String, dynamic> where = const {},
  }) async =>
      (await _send({'kind': 'query', 'model': model, 'filter': where}) as List)
          .cast<Map<String, dynamic>>();
  Future<List<Map<String, dynamic>>> readSql(
    String sql, {
    List<dynamic> parameters = const [],
  }) async =>
      (await _send({'kind': 'sql', 'sql': sql, 'parameters': parameters})
              as List)
          .cast<Map<String, dynamic>>();
  Future<List<Map<String, dynamic>>> querySpec(
    String model,
    Map<String, dynamic> query,
  ) async =>
      (await _send({'kind': 'querySpec', 'model': model, 'query': query})
              as List)
          .cast<Map<String, dynamic>>();
  Future<Map<String, dynamic>?> related(
    String model,
    Map<String, dynamic> identity,
    String relation,
  ) async =>
      await _send({
            'kind': 'related',
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
                'kind': 'referencing',
                'key': {'model': model, 'identity': identity},
                'source': source,
                'relation': relation,
              })
              as List)
          .cast<Map<String, dynamic>>();
  Future<void> direct(Map<String, dynamic> operation) async {
    await _send({'kind': 'direct', 'operation': operation});
  }

  Future<T> savepoint<T>(Future<T> Function() body) {
    if (!_open) return Future.error(StateError('transaction_closed'));
    if (_active != null && Zone.current[_zoneKey] != _active) {
      _structural = StateError('overlapping savepoints');
      return Future.error(_structural!);
    }
    final parent = _active;
    final parentScope = _scopeOf(Zone.current);
    final token = Object();
    _active = token;
    final failure = _failure;
    final run = runZoned(() async {
      // The savepoint opens in its parent's scope; Rust answers the scope its
      // own commands, release and rollback carry. A token keeps its scope after
      // it closes, so a late command from its zone is refused by Rust.
      final opened =
          await _queue({'kind': 'savepoint'}, parentScope)
              as Map<String, dynamic>;
      final scope = opened['scope'] as String;
      _scopeTokens[token] = scope;
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
        await _queue({'kind': 'release', 'scope': scope}, scope);
        return result;
      } catch (error, stack) {
        await _tail;
        if (_open && _structural == null) {
          if (_active != token) {
            _structural = StateError('unawaited nested savepoint');
            throw _structural!;
          }
          await _queue({'kind': 'rollbackSavepoint', 'scope': scope}, scope);
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
