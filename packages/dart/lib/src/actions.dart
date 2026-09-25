import 'dart:async';

enum ActionStatus { pending, succeeded, failed }

/// A failed execution or an inability to observe its final result.
final class ActionError implements Exception {
  final String code;
  final String execution;
  final Object? cause;
  const ActionError(this.code, {this.execution = 'unknown', this.cause});

  @override
  String toString() => 'ActionError($code, execution: $execution)';
}

sealed class ActionOutcome<T> {
  const ActionOutcome();
}

final class ActionSuccess<T> extends ActionOutcome<T> {
  final T result;
  const ActionSuccess(this.result);
}

final class ActionFailure<T> extends ActionOutcome<T> {
  final ActionError error;
  const ActionFailure(this.error);
}

/// Base of the generated per-Action `store` selectors. A selector chooses
/// which explicit Model outputs also update local Models; results are the
/// same either way.
abstract class ActionStore {
  const ActionStore();

  /// The wire form beside business args: null for the default (store all),
  /// false for none, or a map of output names to booleans.
  Object? toWire();
}

abstract interface class ActionCall<T> {
  ActionStatus get status;
  Future<ActionOutcome<T>> wait();
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
  void fail(ActionError error);
}

final class _CallState<T> implements _PendingState {
  final T Function(dynamic) decode;
  final void Function(_PendingState) retain;
  final Completer<ActionOutcome<T>> _done = Completer<ActionOutcome<T>>();
  ActionStatus status = ActionStatus.pending;

  _CallState(this.decode, this.retain);

  Future<ActionOutcome<T>> wait() {
    if (!_done.isCompleted) retain(this);
    return _done.future;
  }

  @override
  void complete(Map<String, dynamic> outcome) {
    if (_done.isCompleted) return;
    if (outcome['status'] == 'succeeded') {
      try {
        final value = decode(outcome['result']);
        status = ActionStatus.succeeded;
        _done.complete(ActionSuccess<T>(value));
      } catch (error) {
        fail(ActionError('action.observation_failed', cause: error));
      }
      return;
    }
    fail(
      ActionError(
        outcome['code'] as String? ?? 'action.failed',
        execution: outcome['execution'] as String? ?? 'rejected',
      ),
    );
  }

  @override
  void fail(ActionError error) {
    if (_done.isCompleted) return;
    status = ActionStatus.failed;
    _done.complete(ActionFailure<T>(error));
  }
}

final class _ActionHandle<T> implements ActionCall<T> {
  final _CallState<T> _state;
  _ActionHandle(this._state);
  @override
  ActionStatus get status => _state.status;
  @override
  Future<ActionOutcome<T>> wait() => _state.wait();
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

  ActionCall<T> register<T>(String callId, T Function(dynamic) decode) {
    if (_closed) {
      final state = _CallState<T>(decode, (_) {});
      state.fail(const ActionError('client.closed'));
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
      state.fail(const ActionError('client.closed'));
    }
  }
}
