import 'dart:async';
import 'dart:convert';
import 'dart:math';
import 'live.dart';

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

/// The controls every lane offers; the push connection fans them out to the live lane.
abstract class LaneControls {
  Future<void> pause();
  Future<void> resume();
  Future<void> wake();
  Future<void> close();
}

/// Rust owns scheduling; this class supplies timers and cancellable network waits.
class RuntimeConnection implements LaneControls {
  LaneControls? _live;
  void Function()? _invalidateLive;
  void attachLive(LaneControls live, void Function() invalidate) {
    _live = live;
    _invalidateLive = invalidate;
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
    _invalidateLive?.call();
    await _live?.pause();
    await _command('pause');
    try {
      await _activeSync;
    } catch (_) {}
    _notify();
  }

  Future<void> resume() async {
    if (_stopped) return;
    await _live?.resume();
    _paused = false;
    await _command('resume');
    _notify();
  }

  Future<void> wake() async {
    if (_stopped) return;
    await _live?.wake();
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
    _invalidateLive?.call();
    await _live?.close();
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

typedef LiveCommand =
    Future<List<dynamic>> Function(Map<String, dynamic> event);

class _Session {
  final int epoch;
  final Completer<void> abort = Completer<void>();
  bool ended = false;
  _Session(this.epoch);
}

/// Host loop of the live lane. Rust owns the session: which channels, when to
/// catch up, what a page means, when to retry. This loop feeds it events and
/// executes its actions with sockets, HTTP, timers and the credential refresh.
class LiveLane implements LaneControls {
  final LiveCommand _command;
  final ServerSession _network;
  final void Function(Object)? onError;
  final Future<void> Function()? refreshAuth;
  final void Function() _wakePush;
  bool _stopped = false;
  _Session? _session;
  Timer? _timer;
  LiveLane._(
    this._command,
    this._network,
    this.onError,
    this.refreshAuth,
    this._wakePush,
  );
  static Future<LiveLane> start({
    required LiveCommand command,
    required ServerSession network,
    required void Function() wakePush,
    void Function(Object)? onError,
    Future<void> Function()? refreshAuth,
  }) async {
    final lane = LiveLane._(command, network, onError, refreshAuth, wakePush);
    await lane._dispatch({'event': 'start'});
    return lane;
  }

  void _clearTimer() {
    _timer?.cancel();
    _timer = null;
  }

  void _abandon(_Session current) {
    current.ended = true;
    if (!current.abort.isCompleted) current.abort.complete();
  }

  Future<void> _dispatch(Map<String, dynamic> event) async {
    if (_stopped && event['event'] != 'stop') return;
    _clearTimer();
    List<dynamic> actions;
    try {
      actions = await _command(event);
    } catch (error) {
      if (_stopped) return;
      onError?.call(error);
      // A command that failed on an event of the session ends that session.
      final epoch = event['epoch'];
      if (epoch is int &&
          event['event'] != 'closed' &&
          _session?.epoch == epoch) {
        final current = _session!;
        _session = null;
        _abandon(current);
        await _dispatch({'event': 'closed', 'epoch': epoch});
      }
      return;
    }
    for (final action in actions) {
      _execute(action as Map<String, dynamic>);
    }
  }

  Future<void> _fail(_Session current, Object error) async {
    if (current.ended || _stopped) return;
    _abandon(current);
    if (identical(_session, current)) _session = null;
    onError?.call(error);
    if (error is AuthenticationExpired && refreshAuth != null) {
      try {
        await refreshAuth!();
      } catch (refreshError) {
        onError?.call(refreshError);
      }
    }
    await _dispatch({'event': 'closed', 'epoch': current.epoch});
  }

  void _execute(Map<String, dynamic> action) {
    switch (action['type']) {
      case 'open':
        final current = _Session(action['epoch'] as int);
        _session = current;
        _network.open(
          action['subscribe'] as String,
          current.abort.future,
          SocketEvents(
            message: (text) => _dispatch({
              'event': 'message',
              'epoch': current.epoch,
              'body': text,
            }),
            overflow: () =>
                _dispatch({'event': 'overflow', 'epoch': current.epoch}),
            closed: (error, _) => unawaited(_fail(current, error)),
          ),
        );
      case 'request':
        final current = _session;
        if (current == null ||
            current.epoch != action['epoch'] ||
            current.ended) {
          return;
        }
        unawaited(
          _network
              .pull(action['body'] as String, current.abort.future)
              .then(
                (text) => _dispatch({
                  'event': 'catchUp',
                  'epoch': current.epoch,
                  'body': text,
                }),
                onError: (Object error) => _fail(current, error),
              ),
        );
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
      case 'wait':
        _clearTimer();
        _timer = Timer(Duration(milliseconds: action['millis'] as int), () {
          _timer = null;
          unawaited(_dispatch({'event': 'next'}));
        });
    }
  }

  /// Abandon the current session's socket and request now; Rust learns of it
  /// on the next event.
  void cancel() {
    final current = _session;
    if (current != null) _abandon(current);
  }

  @override
  Future<void> pause() async {
    if (_stopped) return;
    cancel();
    await _dispatch({'event': 'pause'});
  }

  @override
  Future<void> resume() async {
    if (_stopped) return;
    await _dispatch({'event': 'resume'});
  }

  @override
  Future<void> wake() async {
    if (_stopped) return;
    await _dispatch({'event': 'wake'});
    // A session the host abandoned that Rust still holds (the change it was
    // abandoned for did not commit, or changed nothing) is reported closed.
    final current = _session;
    if (current != null && current.ended) {
      _session = null;
      await _dispatch({'event': 'closed', 'epoch': current.epoch});
    }
  }

  @override
  Future<void> close() async {
    if (_stopped) return;
    _stopped = true;
    cancel();
    _session = null;
    _clearTimer();
    await _dispatch({'event': 'stop'});
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
