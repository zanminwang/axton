import 'dart:async';
import 'dart:convert';
import 'dart:math';
import 'live.dart';
import 'subscriptions.dart';

typedef Transport = Future<String> Function(String kind, String body);
typedef DirectCarrier =
    Future<String> Function(String body, Future<void> cancellation);
typedef ConnectionControl =
    Future<dynamic> Function(String event, int now, int entropy);

class ActionTransportException implements Exception {
  final String code;
  final String execution = 'unknown';
  final Object? cause;
  ActionTransportException(this.code, [this.cause]);
  @override
  String toString() => code;
}

/// The controls every lane offers; the push connection fans them out to the downlink lane.
abstract class LaneControls {
  Future<void> pause();
  Future<void> resume();
  Future<void> wake();
  Future<void> close();
}

/// Rust owns scheduling; this class supplies timers and cancellable network waits.
class RuntimeConnection implements LaneControls {
  LaneControls? _downlink;
  void Function()? _invalidateDownlink;
  void attachDownlink(LaneControls downlink, void Function() invalidate) {
    _downlink = downlink;
    _invalidateDownlink = invalidate;
  }

  final ConnectionControl _control;
  final Future<void> Function(Transport) _sync;
  final Transport _transport;
  final DirectCarrier? _directCarrier;
  final void Function(Object)? onError;
  final Future<void> Function()? refreshAuth;
  final Duration directTimeout;
  final _directRequests = <Completer<String>, Completer<void>>{};
  final _requests = <Completer<String>>{};
  final _closed = Completer<void>();
  Future<void>? _activeSync;
  bool _paused = false;
  bool _stopped = false;
  int _epoch = 0;
  Completer<void>? _wake;
  Timer? _timer;
  RuntimeConnection._(
    this._control,
    this._sync,
    this._transport,
    this._directCarrier,
    this.onError,
    this.refreshAuth,
    this.directTimeout,
  );
  Future<void> get closed => _closed.future;
  bool get directAvailable => !_stopped;
  static Future<RuntimeConnection> start({
    required ConnectionControl control,
    required Future<void> Function(Transport) sync,
    required Transport transport,
    DirectCarrier? directCarrier,
    void Function(Object)? onError,
    Future<void> Function()? refreshAuth,
    Duration directTimeout = const Duration(seconds: 30),
  }) async {
    if (directTimeout.inMicroseconds <= 0)
      throw ArgumentError.value(directTimeout, 'directTimeout');
    final connection = RuntimeConnection._(
      control,
      sync,
      transport,
      directCarrier,
      onError,
      refreshAuth,
      directTimeout,
    );
    await connection._command('start');
    unawaited(
      connection._loop().catchError((Object error) {
        if (!connection._stopped) connection.onError?.call(error);
      }),
    );
    return connection;
  }

  Future<dynamic> _command(String event) => _control(
    event,
    DateTime.now().millisecondsSinceEpoch,
    Random().nextInt(0x100000000),
  );
  void _notify() {
    _epoch++;
    _timer?.cancel();
    if (_wake?.isCompleted == false) _wake!.complete();
    _wake = null;
  }

  Future<void> _wait(int? millis) {
    final wake = Completer<void>();
    _wake = wake;
    if (millis != null)
      _timer = Timer(Duration(milliseconds: millis), () {
        if (!wake.isCompleted) wake.complete();
        if (identical(_wake, wake)) _wake = null;
      });
    return wake.future;
  }

  Future<String> _request(String kind, String body) {
    if (_stopped || _paused)
      return Future.error(StateError('connection_paused_or_closed'));
    final cancellation = Completer<String>();
    _requests.add(cancellation);
    return Future.any([
      Future.sync(() => _transport(kind, body)),
      cancellation.future,
    ]).whenComplete(() {
      _requests.remove(cancellation);
    });
  }

  /// One bounded direct attempt, including token acquisition and auth refresh.
  Future<String> requestAction(String body) async {
    if (_stopped) throw ActionTransportException('action.unavailable');
    final cancelled = Completer<String>();
    final cancellation = Completer<void>();
    _directRequests[cancelled] = cancellation;
    void terminate(String code) {
      if (!cancellation.isCompleted) cancellation.complete();
      if (!cancelled.isCompleted)
        cancelled.completeError(ActionTransportException(code));
    }

    final timer = Timer(directTimeout, () {
      terminate('action.execution_unknown');
    });
    Future<String> send() async {
      try {
        return await (_directCarrier?.call(body, cancellation.future) ??
            _transport('action', body));
      } on AuthenticationExpired {
        if (refreshAuth == null) rethrow;
        await refreshAuth!();
        if (_stopped || cancelled.isCompleted)
          throw ActionTransportException('action.execution_unknown');
        return _directCarrier?.call(body, cancellation.future) ??
            _transport('action', body);
      }
    }

    try {
      return await Future.any([Future.sync(send), cancelled.future]);
    } on ActionTransportException {
      rethrow;
    } catch (error) {
      throw ActionTransportException('action.execution_unknown', error);
    } finally {
      timer.cancel();
      _directRequests.remove(cancelled);
      if (!cancellation.isCompleted) cancellation.complete();
    }
  }

  void _cancelRequests() {
    for (final request in _requests.toList()) {
      if (!request.isCompleted)
        request.completeError(StateError('connection_paused_or_closed'));
    }
  }

  Future<void> _loop() async {
    while (!_stopped) {
      final observed = _epoch;
      final action = await _command('next') as Map;
      if (_stopped) return;
      if (action['type'] == 'sync') {
        try {
          _activeSync = _sync(_request);
          await _activeSync;
          if (!_stopped) await _command('success');
        } catch (error) {
          if (_stopped) return;
          if (_paused) {
            await _command('success');
            continue;
          }
          onError?.call(error);
          if (error is AuthenticationExpired && refreshAuth != null) {
            try {
              await refreshAuth!();
            } catch (error) {
              onError?.call(error);
            }
          }
          if (!_stopped) await _command('failure');
        } finally {
          _activeSync = null;
        }
      } else {
        if (observed != _epoch) continue;
        await _wait(action['type'] == 'wait' ? action['millis'] as int : null);
      }
    }
  }

  Future<void> pause() async {
    if (_stopped) return;
    _paused = true;
    _cancelRequests();
    _invalidateDownlink?.call();
    await _downlink?.pause();
    await _command('pause');
    try {
      await _activeSync;
    } catch (_) {}
    _notify();
  }

  Future<void> resume() async {
    if (_stopped) return;
    await _downlink?.resume();
    _paused = false;
    await _command('resume');
    _notify();
  }

  Future<void> wake() async {
    if (_stopped) return;
    await _downlink?.wake();
    await _command('wake');
    _notify();
  }

  Future<void> close() async {
    if (_stopped) return;
    _stopped = true;
    for (final entry in _directRequests.entries.toList()) {
      if (!entry.value.isCompleted) entry.value.complete();
      if (!entry.key.isCompleted)
        entry.key.completeError(ActionTransportException('action.unavailable'));
    }
    _cancelRequests();
    _notify();
    _invalidateDownlink?.call();
    await _downlink?.close();
    try {
      await _command('stop');
    } finally {
      _closed.complete();
    }
  }
}

class AuthenticationExpired implements Exception {
  const AuthenticationExpired();
}

typedef DownlinkCommand =
    Future<List<dynamic>> Function(Map<String, dynamic> event);

class _Session {
  final int epoch;
  final Completer<void> abort = Completer<void>();
  bool ended = false;
  _Session(this.epoch);
}

/// Host loop of the downlink lane. Rust owns delivery: which channels, when to
/// catch up, what a page means, when to commit and when to retry. A socket or
/// HTTP callback only enqueues what arrived and wakes this loop; the loop asks
/// Rust to pump and executes what it answers with sockets, HTTP, timers and the
/// credential refresh. Enqueueing answers with no actions, so no page is
/// applied inside a callback.
class DownlinkLane implements LaneControls {
  final DownlinkCommand _command;
  final ServerSession _network;
  final void Function(Object)? onError;
  final Future<void> Function()? refreshAuth;
  final void Function() _wakePush;

  /// Transport state for the subscription status projection
  /// ([subscriptions.dart](subscriptions.dart)); the lane decides nothing here.
  final void Function(DownlinkSignal) _report;
  bool _stopped = false;
  _Session? _session;

  /// Catch-up requests of the open session that have not answered yet.
  int _outstanding = 0;

  /// What abandons the historical pages in flight. They belong to the lane, not
  /// to a socket, so only `pause`, `reset` and `close` abandon them and a
  /// replaced socket leaves them alone
  /// ([#151](https://github.com/zanminwang/axton/issues/151)).
  Completer<void> _loading = Completer<void>();

  /// Every enqueue bumps this and the loop re-checks it before sleeping, so a
  /// wake between the idle decision and the sleep is never lost.
  int _generation = 0;
  Completer<void>? _wake;
  Timer? _timer;
  DownlinkLane._(
    this._command,
    this._network,
    this.onError,
    this.refreshAuth,
    this._wakePush,
    this._report,
  );
  static Future<DownlinkLane> start({
    required DownlinkCommand command,
    required ServerSession network,
    required void Function() wakePush,
    void Function(Object)? onError,
    Future<void> Function()? refreshAuth,
    void Function(DownlinkSignal)? report,
  }) async {
    final lane = DownlinkLane._(
      command,
      network,
      onError,
      refreshAuth,
      wakePush,
      report ?? (_) {},
    );
    await lane._enqueue({'event': 'start'});
    unawaited(
      lane._loop().catchError((Object error) {
        if (!lane._stopped) lane.onError?.call(error);
      }),
    );
    return lane;
  }

  void _notify() {
    _generation++;
    _timer?.cancel();
    _timer = null;
    if (_wake?.isCompleted == false) _wake!.complete();
    _wake = null;
  }

  Future<void> _wait(int? millis) {
    final wake = Completer<void>();
    _wake = wake;
    if (millis != null)
      _timer = Timer(Duration(milliseconds: millis), () {
        if (!wake.isCompleted) wake.complete();
        if (identical(_wake, wake)) _wake = null;
      });
    return wake.future;
  }

  void _abandon(_Session current) {
    current.ended = true;
    if (!current.abort.isCompleted) current.abort.complete();
    // Signals name their session, so reporting one that is already gone is
    // harmless and no path that ends a socket can forget it.
    _report(DownlinkSignal.ended(current.epoch));
  }

  /// Hand Rust one event and wake the loop; Rust answers with no actions.
  Future<void> _enqueue(Map<String, dynamic> event) async {
    if (_stopped && event['event'] != 'stop') return;
    try {
      await _command(event);
    } catch (error) {
      if (!_stopped) onError?.call(error);
    }
    _notify();
  }

  /// The HTTP status a transport error carried, for Rust to tell a refusal the
  /// server decided from a transport failure it must retry.
  int? _statusOf(Object error) => switch (error) {
    PullFailure failure => failure.statusCode,
    AuthenticationExpired _ => 401,
    _ => null,
  };

  /// A 401 the application can clear: refresh once, reporting a failed refresh.
  Future<void> _refresh(Object error) async {
    if (error is! AuthenticationExpired || refreshAuth == null) return;
    try {
      await refreshAuth!();
    } catch (refreshError) {
      onError?.call(refreshError);
    }
  }

  /// One historical page. It rides on no session: its failure ends none, and
  /// the worker decides from the status what the failure means.
  void _load(int id, String body) {
    final cancellation = _loading;
    unawaited(
      _network
          .pull(body, cancellation.future)
          .then(
            (text) =>
                _enqueue({'event': 'response', 'request': id, 'body': text}),
            onError: (Object error) async {
              if (_stopped) return;
              // A page this lane abandoned itself - `pause` - is not the
              // application's failure, as an abandoned session's request is
              // not; the worker is still told, so it clears its slot and asks
              // again on `resume`.
              if (!cancellation.isCompleted) {
                onError?.call(error);
                await _refresh(error);
              }
              await _enqueue({
                'event': 'failed',
                'request': id,
                'reason': error.toString(),
                'status': cancellation.isCompleted ? null : _statusOf(error),
              });
            },
          ),
    );
  }

  /// A socket or request that failed: report it, refresh once, tell Rust.
  Future<void> _fail(
    _Session current,
    Object error,
    Map<String, dynamic> event,
  ) async {
    if (current.ended || _stopped) return;
    _abandon(current);
    if (identical(_session, current)) _session = null;
    onError?.call(error);
    await _refresh(error);
    await _enqueue(event);
  }

  void _execute(Map<String, dynamic> action) {
    switch (action['type']) {
      case 'open':
        final current = _Session(action['epoch'] as int);
        _session = current;
        _outstanding = 0;
        _report(DownlinkSignal.opened(current.epoch));
        _network.open(
          action['subscribe'] as String,
          current.abort.future,
          SocketEvents(
            message: (text) => _enqueue({
              'event': 'message',
              'epoch': current.epoch,
              'body': text,
            }),
            overflow: () =>
                _enqueue({'event': 'overflow', 'epoch': current.epoch}),
            closed: (error, _) => unawaited(
              _fail(current, error, {
                'event': 'closed',
                'epoch': current.epoch,
              }),
            ),
          ),
        );
      case 'request':
        final id = action['request'] as int;
        if (action['bootstrap'] == true) {
          _load(id, action['body'] as String);
          return;
        }
        final current = _session;
        if (current == null || current.ended) return;
        _report(DownlinkSignal.requests(++_outstanding));
        void settled() {
          if (identical(_session, current)) {
            _report(DownlinkSignal.requests(--_outstanding));
          }
        }

        unawaited(
          _network
              .pull(action['body'] as String, current.abort.future)
              .then(
                (text) {
                  settled();
                  return _enqueue({
                    'event': 'response',
                    'request': id,
                    'body': text,
                  });
                },
                onError: (Object error) {
                  settled();
                  return _fail(current, error, {
                    'event': 'failed',
                    'request': id,
                    'reason': error.toString(),
                    'status': _statusOf(error),
                  });
                },
              ),
        );
      // The replica was rebuilt and the worker forgot everything it had in
      // flight: abandon the socket and every page, catch-up and bootstrap
      // alike. It comes first in its batch and replaces a `close`, so it never
      // reaches the session the batch opens next. A local abort, as `pause`
      // is: whatever the old I/O still answers is the application's failure no
      // more, and the worker ignores it by epoch and request id
      // ([#162](https://github.com/zanminwang/axton/issues/162)).
      case 'reset':
        final current = _session;
        if (current != null) _abandon(current);
        _session = null;
        _outstanding = 0;
        _abandonLoads();
        _loading = Completer<void>();
      case 'close':
        if (_session?.epoch == action['epoch']) {
          _abandon(_session!);
          _session = null;
        }
        final reason = action['reason'];
        if (reason != null) onError?.call(StateError(reason as String));
      case 'wake':
        _wakePush();
      case 'report':
        for (final report in action['reports'] as List<dynamic>) {
          onError?.call(AxtonReport.fromJson(report as Map<String, dynamic>));
        }
      // The Scopes a commit moved and the set the handshake covered: the
      // subscription status projection reads both. `wait` is the loop's own
      // sleep.
      case 'changed':
        _report(
          DownlinkSignal.changed((action['scopes'] as List).cast<String>()),
        );
      case 'acknowledged':
        _report(
          DownlinkSignal.acknowledged(
            (action['scopes'] as List).cast<String>(),
          ),
        );
      // A committed bootstrap transition: transport state to project, never a
      // decision to make here.
      case 'bootstrap':
        _report(
          DownlinkSignal.bootstrap({
            for (final field in action.entries)
              if (field.key != 'type') field.key: field.value,
          }),
        );
    }
  }

  Future<void> _loop() async {
    while (!_stopped) {
      final observed = _generation;
      List<dynamic> actions;
      try {
        actions = await _command({'event': 'next'});
      } catch (error) {
        if (_stopped) return;
        onError?.call(error);
        // A pump that failed on the open session ends it; with none open there
        // is nothing to retry until something new arrives.
        final current = _session;
        if (current != null) {
          _session = null;
          _abandon(current);
          await _enqueue({'event': 'closed', 'epoch': current.epoch});
        } else if (observed == _generation) {
          await _wait(null);
        }
        continue;
      }
      if (_stopped) return;
      int? millis;
      for (final action in actions) {
        final map = action as Map<String, dynamic>;
        if (map['type'] == 'wait') millis = map['millis'] as int;
        _execute(map);
      }
      // A socket the host abandoned that Rust still holds (the change it was
      // abandoned for did not commit, or changed nothing) is reported closed.
      final current = _session;
      if (current != null && current.ended) {
        _session = null;
        await _enqueue({'event': 'closed', 'epoch': current.epoch});
        continue;
      }
      // Actions mean the worker made progress: pump again, yielding first, so a
      // commit never waits on a timer.
      if (millis == null && actions.isNotEmpty) continue;
      if (observed != _generation) continue;
      await _wait(millis);
    }
  }

  /// Abandon the current session's socket and request now; Rust learns of it
  /// on the next pump.
  void cancel() {
    final current = _session;
    if (current != null) _abandon(current);
  }

  /// Abandon the historical pages in flight; a new page belongs to a new
  /// cancellation, so an answer to an abandoned one reaches no worker.
  void _abandonLoads() {
    if (!_loading.isCompleted) _loading.complete();
  }

  @override
  Future<void> pause() async {
    if (_stopped) return;
    cancel();
    _abandonLoads();
    _report(const DownlinkSignal.paused());
    await _enqueue({'event': 'pause'});
  }

  @override
  Future<void> resume() async {
    if (_stopped) return;
    _loading = Completer<void>();
    _report(const DownlinkSignal.resumed());
    await _enqueue({'event': 'resume'});
  }

  @override
  Future<void> wake() async {
    if (_stopped) return;
    await _enqueue({'event': 'wake'});
  }

  @override
  Future<void> close() async {
    if (_stopped) return;
    _stopped = true;
    cancel();
    _abandonLoads();
    _session = null;
    _report(const DownlinkSignal.stopped());
    await _enqueue({'event': 'stop'});
    _notify();
  }
}

/// A delivery the client could not apply, handed to `onError`. The client
/// stays consistent: a `readFailed` or `skipped` record keeps its local
/// content and stamp, a `conflict` keeps the local content, a `diverged`
/// mutation shows the server's row and is still sent.
class AxtonReport implements Exception {
  AxtonReport({
    required this.kind,
    required this.model,
    required this.identity,
    required this.stamp,
    this.code,
    this.ordinal,
    this.detail,
  });
  factory AxtonReport.fromJson(Map<String, dynamic> json) => AxtonReport(
    kind: json['kind'] as String,
    model: json['model'] as String,
    identity: Map<String, dynamic>.from(json['identity'] as Map),
    stamp: json['stamp'] as int,
    code: json['code'] as String?,
    ordinal: json['ordinal'] as int?,
    detail: json['detail'],
  );

  /// `readFailed`, `skipped`, `conflict` or `diverged`.
  final String kind;
  final String model;
  final Map<String, dynamic> identity;
  final int stamp;

  /// `readFailed`: the server's code (`loader.failed`, or the refusal code).
  final String? code;

  /// `diverged`: the queued mutation whose replay failed; it is still sent.
  final int? ordinal;
  final Object? detail;

  @override
  String toString() =>
      'AxtonReport($kind: $model ${jsonEncode(identity)} at stamp $stamp'
      '${code == null ? '' : ' ($code)'}'
      '${ordinal == null ? '' : ' (mutation $ordinal)'})';
}
