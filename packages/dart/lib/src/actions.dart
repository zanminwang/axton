import 'dart:async';

enum CallStatus { pending, succeeded, failed }

/// A failed execution or an inability to observe its final result.
final class CallError implements Exception {
  final String code;
  final String execution;
  final Object? cause;
  const CallError(this.code, {this.execution = 'unknown', this.cause});

  @override
  String toString() => 'CallError($code, execution: $execution)';
}

sealed class CallOutcome<T> {
  const CallOutcome();
}

final class CallSuccess<T> extends CallOutcome<T> {
  final T result;
  const CallSuccess(this.result);
}

final class CallFailure<T> extends CallOutcome<T> {
  final CallError error;
  const CallFailure(this.error);
}

/// Base of the generated per-Action `store` selectors. A selector chooses
/// which explicit Model outputs also update local Models; results are the
/// same either way.
abstract class CallStore {
  const CallStore();

  /// The wire form beside business args: null for the default (store all),
  /// false for none, or a map of output names to booleans.
  Object? toWire();
}

abstract interface class Call<T> {
  CallStatus get status;
  Future<CallOutcome<T>> wait();
}

abstract interface class ActionWeakState {
  Object? get target;
}

final class _WeakState implements ActionWeakState {
  final WeakReference<Object> _reference;
  _WeakState(Object state) : _reference = WeakReference(state);
  @override
  Object? get target => _reference.target;
}

abstract class _PendingState {
  void complete(Map<String, dynamic> outcome);
  void fail(CallError error);
}

final class _CallState<T> implements _PendingState {
  final T Function(dynamic) decode;
  final void Function(_PendingState) retain;
  final Completer<CallOutcome<T>> _done = Completer<CallOutcome<T>>();
  CallStatus status = CallStatus.pending;

  _CallState(this.decode, this.retain);

  Future<CallOutcome<T>> wait() {
    if (!_done.isCompleted) retain(this);
    return _done.future;
  }

  @override
  void complete(Map<String, dynamic> outcome) {
    if (_done.isCompleted) return;
    if (outcome['status'] == 'succeeded') {
      try {
        final value = decode(outcome['result']);
        status = CallStatus.succeeded;
        _done.complete(CallSuccess<T>(value));
      } catch (error) {
        fail(CallError('action.observation_failed', cause: error));
      }
      return;
    }
    fail(
      CallError(
        outcome['code'] as String? ?? 'action.failed',
        execution: outcome['execution'] as String? ?? 'rejected',
      ),
    );
  }

  @override
  void fail(CallError error) {
    if (_done.isCompleted) return;
    status = CallStatus.failed;
    _done.complete(CallFailure<T>(error));
  }
}

final class _ActionHandle<T> implements Call<T> {
  final _CallState<T> _state;
  _ActionHandle(this._state);
  @override
  CallStatus get status => _state.status;
  @override
  Future<CallOutcome<T>> wait() => _state.wait();
}

/// Per-client completion routing. Weak slots do not retain abandoned handles.
final class ActionObservers {
  final Map<String, ActionWeakState> _routes = {};
  final Map<String, _PendingState> _active = {};
  final ActionWeakState Function(Object) _weak;
  bool _closed = false;

  ActionObservers({ActionWeakState Function(Object)? weak})
    : _weak = weak ?? _WeakState.new;

  int get routingCount {
    _sweep();
    return _routes.length;
  }

  void _sweep() => _routes.removeWhere((_, ref) => ref.target == null);

  Call<T> register<T>(String callId, T Function(dynamic) decode) {
    if (_closed) {
      final state = _CallState<T>(decode, (_) {});
      state.fail(const CallError('client.closed'));
      return _ActionHandle<T>(state);
    }
    _sweep();
    final state = _CallState<T>(decode, (state) {
      _active[callId] = state;
    });
    _routes[callId] = _weak(state);
    return _ActionHandle<T>(state);
  }

  void complete(Map<String, dynamic> event) {
    _sweep();
    final callId = event['callId'] as String;
    final state =
        _active.remove(callId) ??
        (_routes.remove(callId)?.target as _PendingState?);
    _routes.remove(callId);
    if (state == null) return;
    state.complete((event['outcome'] as Map).cast<String, dynamic>());
  }

  void close() {
    if (_closed) return;
    _closed = true;
    final states = <_PendingState>{..._active.values};
    for (final ref in _routes.values) {
      final state = ref.target;
      if (state is _PendingState) states.add(state);
    }
    _routes.clear();
    _active.clear();
    for (final state in states) {
      state.fail(const CallError('client.closed'));
    }
  }
}
