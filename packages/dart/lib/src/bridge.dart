/// The Dart SDK Bridge over the Rust-owned client runtime
/// ([#134](https://github.com/zanminwang/axton/issues/134)).
///
/// A [Bridge] submits complete tasks through the C ABI
/// (`axton_runtime_open/submit/drain/detach`), answers the effects the
/// runtime asks for, and delivers each `taskCompleted` to the waiter that
/// submitted it. It holds maps and platform resources only: task progression,
/// database scheduling and retry stay in Rust, whose actor thread owns SQLite,
/// so admission and drain never block on the database and everything runs on
/// the calling isolate.
///
/// The actor wakes the bridge through one process-wide
/// `NativeCallable.listener`: Rust calls it from its own thread under the
/// actor's sink lock, the trampoline only posts to this isolate, and the drain
/// runs on this isolate's event loop. See the
/// [bridge contract](../../../../crates/client/src/runtime/protocol.rs).
library;

import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';

import 'package:ffi/ffi.dart';

typedef _WakeNative = Void Function(Uint64 runtime, Pointer<Void> context);
typedef _OpenNative =
    Uint64 Function(
      Pointer<Utf8> request,
      Pointer<NativeFunction<_WakeNative>> wake,
      Pointer<Void> context,
      Pointer<Pointer<Utf8>> errorOut,
    );
typedef _Open =
    int Function(
      Pointer<Utf8> request,
      Pointer<NativeFunction<_WakeNative>> wake,
      Pointer<Void> context,
      Pointer<Pointer<Utf8>> errorOut,
    );
typedef _SubmitNative =
    Int32 Function(
      Uint64 runtime,
      Pointer<Utf8> message,
      Pointer<Pointer<Utf8>> errorOut,
    );
typedef _Submit =
    int Function(
      int runtime,
      Pointer<Utf8> message,
      Pointer<Pointer<Utf8>> errorOut,
    );
typedef _DrainNative = Pointer<Utf8> Function(Uint64 runtime);
typedef _Drain = Pointer<Utf8> Function(int runtime);
typedef _DetachNative = Void Function(Uint64 runtime);
typedef _Detach = void Function(int runtime);
typedef _FreeNative = Void Function(Pointer<Utf8> output);
typedef _Free = void Function(Pointer<Utf8> output);

/// The runtime functions of one loaded library. Every `char*` the library
/// returns is copied into Dart and freed here exactly once.
class _Abi {
  _Abi(DynamicLibrary library)
    : _open = library.lookupFunction<_OpenNative, _Open>('axton_runtime_open'),
      _submit = library.lookupFunction<_SubmitNative, _Submit>(
        'axton_runtime_submit',
      ),
      _drain = library.lookupFunction<_DrainNative, _Drain>(
        'axton_runtime_drain',
      ),
      detach = library.lookupFunction<_DetachNative, _Detach>(
        'axton_runtime_detach',
      ),
      _free = library.lookupFunction<_FreeNative, _Free>('axton_free');

  final _Open _open;
  final _Submit _submit;
  final _Drain _drain;
  final void Function(int runtime) detach;
  final _Free _free;

  static final _loaded = <String?, _Abi>{};

  /// The library at [libraryPath], or the process's own symbols on iOS,
  /// loaded once per isolate.
  static _Abi load(String? libraryPath) {
    final loaded = _loaded[libraryPath];
    if (loaded != null) return loaded;
    if (libraryPath == null && !Platform.isIOS) {
      throw StateError('libraryPath is required outside iOS');
    }
    try {
      return _loaded[libraryPath] = _Abi(
        libraryPath != null
            ? DynamicLibrary.open(libraryPath)
            : DynamicLibrary.process(),
      );
    } catch (error) {
      throw StateError('$error');
    }
  }

  /// Answer the owned text at [output] and free it.
  String _take(Pointer<Utf8> output) {
    try {
      return output.toDartString();
    } finally {
      _free(output);
    }
  }

  /// Run [call] with an error slot; answer its result and the error text the
  /// callee wrote, if any.
  (int, String?) _withError(int Function(Pointer<Pointer<Utf8>>) call) {
    final slot = calloc<Pointer<Utf8>>();
    try {
      final result = call(slot);
      return (result, slot.value == nullptr ? null : _take(slot.value));
    } finally {
      calloc.free(slot);
    }
  }

  /// Open a runtime; its id, or 0 with the reason.
  (int, String?) open(String request, Pointer<NativeFunction<_WakeNative>> w) {
    final text = request.toNativeUtf8();
    try {
      return _withError((error) => _open(text, w, nullptr, error));
    } finally {
      malloc.free(text);
    }
  }

  /// Admit one envelope; null, or why it was refused.
  String? submit(int runtime, String message) {
    final text = message.toNativeUtf8();
    try {
      final (code, error) = _withError(
        (error) => _submit(runtime, text, error),
      );
      return code == 0 ? null : error ?? 'client_closed';
    } finally {
      malloc.free(text);
    }
  }

  /// The events published so far.
  List<dynamic> drain(int runtime) =>
      jsonDecode(_take(_drain(runtime))) as List<dynamic>;
}

/// One submitted input awaiting its `taskCompleted`.
class _Route {
  _Route([this.run, this.zone]);
  final completer = Completer<dynamic>();

  /// The transaction callback of a `transaction` task, and the zone it was
  /// submitted from, where it runs.
  final Future<void> Function(String transactionId)? run;
  final Zone? zone;

  /// What the callback threw, rethrown as it was when the task fails.
  Object? thrown;
  StackTrace? stack;
}

/// Runs one kind of host effect. It answers through the [Effect] and aborts
/// its platform resource when [Effect.cancelled] completes.
typedef EffectHandler = void Function(Effect effect);

/// One effect the runtime asked the host for: its operation, the per-effect
/// cancellation a `cancelEffect` completes, and its answers. Answers after a
/// cancellation are dropped here, and the runtime fences them anyway.
class Effect {
  Effect(this.id, this.operation, this._answer, [this._done]);

  /// The runtime's `effectId`.
  final String id;

  /// The `{kind, ...}` operation to execute.
  final Map<String, dynamic> operation;
  final void Function(Map<String, dynamic> outcome) _answer;
  final void Function()? _done;
  final _cancelled = Completer<void>();
  bool _finished = false;

  /// Completes once the runtime cancelled this effect or its handler was
  /// removed: the platform resource is aborted, and nothing more is answered.
  Future<void> get cancelled => _cancelled.future;
  bool get isCancelled => _cancelled.isCompleted;

  /// The single answer of an HTTP, timer, credential or prerequisite effect.
  void succeed([Object? value]) {
    _send({'ok': true, if (value != null) 'value': value});
    _finish();
  }

  /// One result of a socket stream; the stream goes on.
  void emit(Object value) => _send({'ok': true, 'value': value});

  /// The effect failed, with the HTTP [status] the failure carried, if any.
  /// It ends a socket stream too.
  void fail(String message, {int? status}) {
    _send({
      'ok': false,
      'error': {'message': message, if (status != null) 'status': status},
    });
    _finish();
  }

  /// Abort the platform resource; later answers are dropped.
  void cancel() {
    if (!_cancelled.isCompleted) _cancelled.complete();
    _finish();
  }

  void _send(Map<String, dynamic> outcome) {
    if (!_finished && !isCancelled) _answer(outcome);
  }

  void _finish() {
    if (_finished) return;
    _finished = true;
    _done?.call();
  }
}

/// What the connection and the prerequisite runner need from a runtime: tasks
/// and effect handlers by operation kind. The [Bridge] is one; tests drive the
/// handlers through a fake.
abstract interface class RuntimeHost {
  Future<dynamic> task(Map<String, dynamic> command);

  /// Run every effect of [kind] with [handler] until [stopHandling].
  void handleEffects(String kind, EffectHandler handler);

  /// Remove [handler] if it still handles [kind], aborting every effect of
  /// that kind it still holds.
  void stopHandling(String kind, EffectHandler handler);
}

/// The SDK side of one Rust-owned client runtime.
class Bridge implements RuntimeHost {
  Bridge._(this._abi, this.runtimeId);

  final _Abi _abi;

  /// The runtime's id: fresh per open, never reused.
  final int runtimeId;

  /// What the open answered: `clientId` and the schema check's `schema`.
  late final Map<String, dynamic> opened;

  /// Request ids: decimal strings from one increasing counter, never reused.
  /// The open task is `1`.
  int _requests = 1;
  final _routes = <String, _Route>{};
  bool _draining = false;
  bool _detached = false;
  Future<void>? _closing;
  final _terminated = Completer<void>();
  final _changed = StreamController<List<String>>.broadcast(sync: true);
  final _reports = StreamController<Map<String, dynamic>>.broadcast(sync: true);

  /// The tables of every committed local transaction, before the completion
  /// of the task that committed it. Listener errors go to the listener's zone.
  Stream<List<String>> get changed => _changed.stream;

  /// What the runtime reports that is not a task outcome: the `diagnostic` of
  /// every `report` event.
  Stream<Map<String, dynamic>> get reports => _reports.stream;

  /// `callCompleted`: a durable, direct or abandoned call's final outcome,
  /// after the commit that decided it.
  void Function(String callId, dynamic outcome)? onCallCompleted;

  /// `observerChanged`; no runtime emits it before the later checkpoints of
  /// #134.
  void Function(String observerId, dynamic snapshot)? onObserverChanged;

  /// `laneSignal`: transport state of the connection lanes for the
  /// subscription status projection.
  void Function(Map<String, dynamic> signal)? onLaneSignal;

  /// Effect handlers by operation kind, and the effects they hold by id.
  final _handlers = <String, EffectHandler>{};
  final _effects = <String, Effect>{};

  /// Attached bridges by runtime id: what a wake names.
  static final _bridges = <int, Bridge>{};

  /// Runtime ids of this isolate's attached bridges.
  static Iterable<int> get attached => _bridges.keys;

  /// The one wake callback of this isolate, shared by every runtime and never
  /// closed: a process-wide listener costs one port, and never closing it
  /// means no runtime can ever hold a pointer to a closed callable, however
  /// its detach and a close raced. It does not keep the isolate alive on its
  /// own: while anything waits on a runtime (an open, a task, a close) it
  /// does, so an awaited outcome is always delivered, and a forgotten client
  /// with nothing outstanding pins nothing.
  static NativeCallable<_WakeNative>? _wake;
  static int _held = 0;

  static Pointer<NativeFunction<_WakeNative>> get _wakePointer =>
      (_wake ??= NativeCallable<_WakeNative>.listener(
        _woken,
      )..keepIsolateAlive = _held > 0).nativeFunction;

  static void _woken(int runtime, Pointer<Void> _) =>
      _bridges[runtime]?._drain();

  static void _hold() {
    if (_held++ == 0) _wake?.keepIsolateAlive = true;
  }

  static void _release() {
    if (--_held == 0) _wake?.keepIsolateAlive = false;
  }

  /// Open a runtime for the database at [path] and answer once Rust opened
  /// it. A failed open throws its reason as a [StateError], after the runtime
  /// announced its end and was detached. [migration] is accepted for API
  /// compatibility; the row-based client keeps none.
  static Future<Bridge> open({
    required String path,
    required Map<String, dynamic> schema,
    String? libraryPath,
    Map<String, dynamic>? migration,
    bool discardPending = false,
  }) async {
    final abi = _Abi.load(libraryPath);
    final request = jsonEncode({
      'type': 'open',
      'requestId': '1',
      'path': path,
      'schema': schema,
      'discardPending': discardPending,
    });
    final (runtime, refused) = abi.open(request, _wakePointer);
    if (runtime == 0) throw StateError(refused ?? 'runtime open failed');
    // Registered before any wake can be delivered: the listener only posts to
    // this isolate, which runs it after this synchronous section.
    final bridge = Bridge._(abi, runtime);
    final route = _Route();
    bridge._routes['1'] = route;
    _hold();
    _bridges[runtime] = bridge;
    try {
      bridge.opened = (await route.completer.future) as Map<String, dynamic>;
    } catch (_) {
      // A failed open closed its runtime; wait for its end and detach.
      _hold();
      try {
        await bridge._terminated.future;
      } finally {
        _release();
      }
      rethrow;
    }
    return bridge;
  }

  /// Submit one task and answer its value, or throw its error as a
  /// [StateError] carrying the engine's message (`client_closed` once the
  /// runtime is gone).
  @override
  Future<dynamic> task(Map<String, dynamic> command) =>
      _route(_Route(), (id) => taskEnvelope(id, command));

  @override
  void handleEffects(String kind, EffectHandler handler) =>
      _handlers[kind] = handler;

  @override
  void stopHandling(String kind, EffectHandler handler) {
    if (!identical(_handlers[kind], handler)) return;
    _handlers.remove(kind);
    for (final effect in _effects.values.toList()) {
      if (effect.operation['kind'] == kind) effect.cancel();
    }
  }

  /// Run [run] as the callback of one local transaction the runtime owns.
  /// Ordinary tasks wait while it runs; its own commands go through
  /// [transactionCommand] with the id it is given. Completes once Rust
  /// committed; otherwise throws what [run] threw, or the runtime's reason.
  Future<void> transaction(Future<void> Function(String transactionId) run) =>
      _route(
        _Route(run, Zone.current),
        (id) => taskEnvelope(id, const {'kind': 'transaction'}),
      );

  /// One command of the open callback transaction [transactionId], in the
  /// savepoint [scope] (null at the top level).
  Future<dynamic> transactionCommand(
    String transactionId,
    String? scope,
    Map<String, dynamic> command,
  ) => _route(
    _Route(),
    (id) => transactionCommandEnvelope(id, transactionId, scope, command),
  );

  /// Answer one effect. A refusal means the runtime is gone, which fences the
  /// effect anyway.
  void effectResult(
    String effectId, {
    required bool ok,
    Object? value,
    String? error,
    int? status,
  }) => _submitQuietly(
    effectResultEnvelope(
      effectId,
      ok: ok,
      value: value,
      error: error,
      status: status,
    ),
  );

  /// Test seam: admit a raw envelope and answer the refusal, if any.
  String? submitRaw(Map<String, dynamic> envelope) => _detached
      ? 'client_closed'
      : _abi.submit(runtimeId, jsonEncode(envelope));

  /// Close the runtime: every waiter settles (`client_closed` for what did not
  /// complete), the runtime detaches, and this completes. Idempotent.
  Future<void> close() => _closing ??= _close();

  Future<void> _close() async {
    if (_terminated.isCompleted) return;
    _hold();
    try {
      // A refusal means the runtime already closed itself: its
      // `runtimeClosed` is dispatched or on its way.
      _submitQuietly(closeEnvelope);
      await _terminated.future;
    } finally {
      _release();
    }
  }

  /// Register the route before admission; an admission refusal removes and
  /// fails it.
  Future<dynamic> _route(
    _Route route,
    Map<String, dynamic> Function(String requestId) envelope,
  ) {
    if (_detached) return Future.error(StateError('client_closed'));
    final requestId = '${++_requests}';
    final String message;
    try {
      message = jsonEncode(envelope(requestId));
    } catch (error, stack) {
      return Future.error(error, stack);
    }
    _routes[requestId] = route;
    _hold();
    final refused = _abi.submit(runtimeId, message);
    if (refused != null && identical(_routes.remove(requestId), route)) {
      _release();
      route.completer.completeError(StateError(refused));
    }
    return route.completer.future;
  }

  void _submitQuietly(Map<String, dynamic> envelope) {
    if (!_detached) _abi.submit(runtimeId, jsonEncode(envelope));
  }

  /// Drain until empty, dispatching every event in order. Never re-entrant: a
  /// wake that arrives while draining is covered by the loop.
  void _drain() {
    if (_draining) return;
    _draining = true;
    try {
      while (!_detached) {
        final batch = _abi.drain(runtimeId);
        if (batch.isEmpty) return;
        for (final event in batch) {
          if (_detached) return;
          try {
            _dispatch(event as Map<String, dynamic>);
          } catch (error, stack) {
            Zone.current.handleUncaughtError(error, stack);
          }
        }
      }
    } finally {
      _draining = false;
    }
  }

  void _dispatch(Map<String, dynamic> event) {
    switch (event['type']) {
      case 'taskCompleted':
        _complete(event);
      case 'effect':
        _effect(
          event['effectId'] as String,
          event['operation'] as Map<String, dynamic>,
        );
      case 'cancelEffect':
        _effects[event['effectId']]?.cancel();
      case 'changed':
        _changed.add((event['tables'] as List).cast<String>());
      case 'report':
        _reports.add(event['diagnostic'] as Map<String, dynamic>);
      case 'callCompleted':
        onCallCompleted?.call(event['callId'] as String, event['outcome']);
      case 'observerChanged':
        onObserverChanged?.call(
          event['observerId'] as String,
          event['snapshot'],
        );
      case 'laneSignal':
        onLaneSignal?.call(event['signal'] as Map<String, dynamic>);
      case 'runtimeClosed':
        _terminate();
    }
  }

  /// Remove the route, then settle it exactly once.
  void _complete(Map<String, dynamic> event) {
    final route = _routes.remove(event['requestId']);
    if (route == null) return;
    _release();
    if (event['ok'] == true) {
      route.completer.complete(event['value']);
    } else if (route.thrown != null) {
      route.completer.completeError(route.thrown!, route.stack);
    } else {
      route.completer.completeError(
        StateError(event['error'] as String? ?? 'task failed'),
      );
    }
  }

  void _effect(String effectId, Map<String, dynamic> operation) {
    if (operation['kind'] != 'callback') {
      final effect = Effect(
        effectId,
        operation,
        (outcome) => _submitQuietly(effectOutcomeEnvelope(effectId, outcome)),
        () => _effects.remove(effectId),
      );
      final handler = _handlers[operation['kind']];
      if (handler == null) {
        effect.fail('unsupported effect');
        return;
      }
      _effects[effectId] = effect;
      try {
        handler(effect);
      } catch (error) {
        effect.fail('$error');
      }
      return;
    }
    final transactionId = operation['transactionId'] as String;
    final route = _routes[operation['requestId']];
    final run = route?.run;
    if (route == null || run == null) {
      _submitQuietly(
        callbackResultEnvelope(
          effectId,
          transactionId,
          ok: false,
          error: 'no transaction callback',
        ),
      );
      return;
    }
    // Application code runs after this batch is dispatched, in the zone the
    // transaction was submitted from; the task completes only from Rust.
    route.zone!.scheduleMicrotask(() {
      Future<void>.sync(() => run(transactionId)).then(
        (_) => _submitQuietly(
          callbackResultEnvelope(effectId, transactionId, ok: true),
        ),
        onError: (Object error, StackTrace stack) {
          route
            ..thrown = error
            ..stack = stack;
          _submitQuietly(
            callbackResultEnvelope(
              effectId,
              transactionId,
              ok: false,
              error: error.toString(),
            ),
          );
        },
      );
    });
  }

  /// `runtimeClosed`: fail what never completed, detach (after which no wake
  /// runs for this id), forget the id and end the streams.
  void _terminate() {
    if (_detached) return;
    _detached = true;
    _abi.detach(runtimeId);
    _bridges.remove(runtimeId);
    for (final effect in _effects.values.toList()) {
      effect.cancel();
    }
    _handlers.clear();
    final remaining = _routes.values.toList();
    _routes.clear();
    for (final route in remaining) {
      _release();
      route.completer.completeError(StateError('client_closed'));
    }
    unawaited(_changed.close());
    unawaited(_reports.close());
    _terminated.complete();
  }

  static Map<String, dynamic> taskEnvelope(
    String requestId,
    Map<String, dynamic> command,
  ) => {'type': 'task', 'requestId': requestId, 'command': command};

  static Map<String, dynamic> transactionCommandEnvelope(
    String requestId,
    String transactionId,
    String? scope,
    Map<String, dynamic> command,
  ) => {
    'type': 'transactionCommand',
    'requestId': requestId,
    'transactionId': transactionId,
    if (scope != null) 'scope': scope,
    'command': command,
  };

  static Map<String, dynamic> callbackResultEnvelope(
    String effectId,
    String transactionId, {
    required bool ok,
    String? error,
  }) => {
    'type': 'callbackResult',
    'effectId': effectId,
    'transactionId': transactionId,
    'ok': ok,
    if (error != null) 'error': error,
  };

  static Map<String, dynamic> effectResultEnvelope(
    String effectId, {
    required bool ok,
    Object? value,
    String? error,
    int? status,
  }) => {
    'type': 'effectResult',
    'effectId': effectId,
    'outcome': {
      'ok': ok,
      if (value != null) 'value': value,
      if (error != null)
        'error': {'message': error, if (status != null) 'status': status},
    },
  };

  static Map<String, dynamic> effectOutcomeEnvelope(
    String effectId,
    Map<String, dynamic> outcome,
  ) => {'type': 'effectResult', 'effectId': effectId, 'outcome': outcome};

  static const Map<String, dynamic> closeEnvelope = {'type': 'close'};
}
