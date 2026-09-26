/// Subscription handles: the identity a registration keeps, the status it
/// publishes and the observers watching it
/// ([#150](https://github.com/zanminwang/axton/issues/150)). A handle owns none
/// of the synchronization: Rust decides what is delivered and when, and this
/// file only projects what it committed and what its lane is doing onto one
/// immutable snapshot per subscription.
library;

import 'dart:async';

/// Whether a durable starting boundary exists for a subscription. It says
/// nothing about the connection.
enum SubscriptionInitialization { pending, ready }

/// What the lane is doing for a subscription. `live` means the session
/// delivers normally, not that all history is loaded.
enum SubscriptionConnection { offline, connecting, catchingUp, live, stopped }

/// What the durable load of a Scope's published history is doing
/// ([#151](https://github.com/zanminwang/axton/issues/151)).
/// `waitingForInitialization` is a requested run with no starting boundary to
/// bound its interval yet, `catchingUp` a loaded interval whose completion
/// barrier ordinary delivery has not reached, and `complete` says the initial
/// publication coverage was processed - not that a snapshot was taken, nor that
/// the Scope is currently fresh.
enum BootstrapPhase {
  notRequested,
  waitingForInitialization,
  loading,
  catchingUp,
  complete,
  failed,
}

/// The stored failure of a load, as its status publishes it. The failed page's
/// record reports stay in the ledger and are not part of this.
class BootstrapError {
  final String code;
  final String message;
  const BootstrapError({required this.code, required this.message});
  @override
  bool operator ==(Object other) =>
      other is BootstrapError && other.code == code && other.message == message;
  @override
  int get hashCode => Object.hash(code, message);
  @override
  String toString() => 'BootstrapError($code: $message)';
}

/// One immutable snapshot of a subscription's durable load.
class BootstrapStatus {
  final BootstrapPhase phase;
  final BootstrapError? error;
  const BootstrapStatus({required this.phase, this.error});
  @override
  bool operator ==(Object other) =>
      other is BootstrapStatus && other.phase == phase && other.error == error;
  @override
  int get hashCode => Object.hash(phase, error);
  @override
  String toString() => 'BootstrapStatus(${phase.name}, error: $error)';
}

/// One immutable subscription status snapshot.
class SubscriptionStatus {
  /// Whether this handle still names a live registration.
  final bool active;
  final SubscriptionInitialization initialization;
  final SubscriptionConnection connection;

  /// The durable load of this Scope's published history, as it was last
  /// committed.
  final BootstrapStatus bootstrap;
  const SubscriptionStatus({
    required this.active,
    required this.initialization,
    required this.connection,
    this.bootstrap = const BootstrapStatus(phase: BootstrapPhase.notRequested),
  });
  @override
  bool operator ==(Object other) =>
      other is SubscriptionStatus &&
      other.active == active &&
      other.initialization == initialization &&
      other.connection == connection &&
      other.bootstrap == bootstrap;
  @override
  int get hashCode =>
      Object.hash(active, initialization, connection, bootstrap);
  @override
  String toString() =>
      'SubscriptionStatus(active: $active, '
      'initialization: ${initialization.name}, '
      'connection: ${connection.name}, '
      'bootstrap: $bootstrap)';
}

/// Work attempted through a handle that is closed: unsubscribed, or stopped
/// with its client.
class SubscriptionClosedException implements Exception {
  final String code = 'subscription.closed';
  const SubscriptionClosedException();
  @override
  String toString() => 'subscription.closed';
}

/// This process stopped waiting because its client closed. The durable task is
/// untouched: a reopened client resumes it without a new call.
class ClientClosedException implements Exception {
  final String code = 'client_closed';
  const ClientClosedException();
  @override
  String toString() => 'client_closed';
}

/// A load run failed, and this is the failure the ledger stored for it. A later
/// explicit call retries that run; this one stays failed.
class BootstrapFailedException implements Exception {
  final String code;
  final String message;
  const BootstrapFailedException(this.code, this.message);
  @override
  String toString() => '$code: $message';
}

/// A later run of the same registration was observed than the one this call is
/// attached to: its own outcome can no longer be observed, and a waiter never
/// completes from another run's, so a rapid retry cannot turn an earlier failed
/// call into a success ([#151](https://github.com/zanminwang/axton/issues/151)).
class BootstrapSupersededException implements Exception {
  final String code = 'bootstrap.superseded';
  final String scope;
  const BootstrapSupersededException(this.scope);
  @override
  String toString() =>
      'bootstrap.superseded: the bootstrap run of $scope this call waited for '
      'was superseded';
}

/// The stable prefix the engine refuses a registration this client no longer
/// holds with (`axton_client::SUBSCRIPTION_CLOSED`). An engine error carries a
/// message and no code, so this is what a closed registration is recognized by;
/// a `bootstrap()` that raced the removal then fails the way a call through an
/// already closed handle does.
const _closedRegistration = 'subscription.closed:';
bool _refusedAsClosed(Object error) =>
    (error is StateError ? error.message : '$error').contains(
      _closedRegistration,
    );

/// One registration's durable load, as the native commands answer it and the
/// worker announces it after every committed transition. `cursor` is how far
/// the historical interval has been loaded and `barrier` the delivery position
/// completion waits for, fixed by the final historical page.
class BootstrapRun {
  final String scope;
  final int subscriptionId;

  /// The stored column value: `not_requested`, `requested`, `loading`,
  /// `catching_up`, `complete` or `failed`.
  final String state;

  /// The retry fence: every call and every response belongs to one run.
  final int run;
  final int cursor;
  final int? barrier;
  final BootstrapError? error;
  const BootstrapRun({
    required this.scope,
    required this.subscriptionId,
    required this.state,
    required this.run,
    required this.cursor,
    this.barrier,
    this.error,
  });
  factory BootstrapRun.fromRecord(Map<String, dynamic> record) {
    final failure = record['error'] as Map<String, dynamic>?;
    return BootstrapRun(
      scope: record['scope'] as String,
      subscriptionId: record['subscriptionId'] as int,
      state: record['state'] as String,
      run: record['run'] as int,
      cursor: record['cursor'] as int,
      barrier: record['barrier'] as int?,
      error: failure == null
          ? null
          : BootstrapError(
              code: failure['code'] as String,
              message: failure['message'] as String,
            ),
    );
  }

  /// How far this run has got. A phase never moves backwards within one run.
  int get rank => switch (state) {
    'requested' => 1,
    'loading' => 2,
    'catching_up' => 3,
    'complete' || 'failed' => 4,
    _ => 0,
  };

  /// The public phase of this stored state. The worker writes `loading` on the
  /// first applied page, so a requested run whose interval is already bounded
  /// is loading as far as the caller is concerned; one without a starting
  /// boundary is waiting for #150 initialization.
  BootstrapPhase phase({required bool initialized}) => switch (state) {
    'requested' =>
      initialized
          ? BootstrapPhase.loading
          : BootstrapPhase.waitingForInitialization,
    'loading' => BootstrapPhase.loading,
    'catching_up' => BootstrapPhase.catchingUp,
    'complete' => BootstrapPhase.complete,
    'failed' => BootstrapPhase.failed,
    _ => BootstrapPhase.notRequested,
  };
}

/// One stored subscription, as the native Scope commands answer it. A boundary
/// that is not committed yet is `null`; zero is a delivery position.
class SubscriptionState {
  final String scope;
  final int subscriptionId;
  final int? startingCursor;
  final int? cursor;
  const SubscriptionState({
    required this.scope,
    required this.subscriptionId,
    this.startingCursor,
    this.cursor,
  });
  factory SubscriptionState.fromRecord(Map<String, dynamic> record) =>
      SubscriptionState(
        scope: record['scope'] as String,
        subscriptionId: record['subscriptionId'] as int,
        startingCursor: record['startingCursor'] as int?,
        cursor: record['cursor'] as int?,
      );
}

/// What the downlink lane tells the registry. It is transport state the lane
/// already has, not a second sync state machine: which session is open,
/// whether its handshake covered a Scope, how many catch-up requests are out,
/// and which Scopes a commit moved. Signals name their session's epoch, so
/// whatever an abandoned session reports changes nothing.
class DownlinkSignal {
  final String lane;
  final int? epoch;
  final int? outstanding;
  final List<String> scopes;

  /// One registration's durable load, as the worker announces it after every
  /// committed transition: `scope`, `subscriptionId`, `state`, `run`, `cursor`,
  /// `barrier` and `error`
  /// ([#151](https://github.com/zanminwang/axton/issues/151)).
  final Map<String, dynamic>? run;
  const DownlinkSignal._(
    this.lane, {
    this.epoch,
    this.outstanding,
    this.scopes = const [],
    this.run,
  });
  const DownlinkSignal.opened(int epoch) : this._('opened', epoch: epoch);
  const DownlinkSignal.ended(int epoch) : this._('ended', epoch: epoch);
  const DownlinkSignal.paused() : this._('paused');
  const DownlinkSignal.resumed() : this._('resumed');
  const DownlinkSignal.stopped() : this._('stopped');
  const DownlinkSignal.requests(int outstanding)
    : this._('requests', outstanding: outstanding);
  const DownlinkSignal.acknowledged(List<String> scopes)
    : this._('acknowledged', scopes: scopes);
  const DownlinkSignal.changed(List<String> scopes)
    : this._('changed', scopes: scopes);
  const DownlinkSignal.bootstrap(Map<String, dynamic> run)
    : this._('bootstrap', run: run);
}

/// The native commands and host services the registry needs; the client owns
/// the serialized command path.
class SubscriptionCommands {
  final Future<SubscriptionState> Function(String scope) subscribe;
  final Future<SubscriptionState?> Function(String scope) state;

  /// Remove exactly the registration this identity names; `true` when a row went.
  final Future<bool> Function(String scope, int subscriptionId) remove;

  /// Remove whatever registration a Scope name has, in one command, so calls
  /// for one Scope keep their order.
  final Future<void> Function(String scope) removeScope;

  /// Register or explicitly retry the durable load of this identity, and answer
  /// its stored run.
  final Future<BootstrapRun> Function(String scope, int subscriptionId)
  requestBootstrap;

  /// The stored run of this identity, read through the same serialized path.
  final Future<BootstrapRun> Function(String scope, int subscriptionId)
  bootstrapState;

  /// A committed membership change: wake the lanes, as every commit does.
  final void Function() committed;
  const SubscriptionCommands({
    required this.subscribe,
    required this.state,
    required this.remove,
    required this.removeScope,
    required this.requestBootstrap,
    required this.bootstrapState,
    required this.committed,
  });
}

/// The load commands of one identity, bound by the registry, and the wake a
/// commit owes the lanes.
class _Load {
  final Future<BootstrapRun> Function() request;
  final Future<BootstrapRun> Function() read;
  final void Function() committed;
  const _Load(this.request, this.read, this.committed);
}

/// One caller of `bootstrap()`, attached to the run the command answered with.
class _Waiter {
  final int run;
  final Completer<void> completer = Completer<void>();
  _Waiter(this.run);
}

/// The lane state every handle's connection is projected from.
class _Lane {
  bool attached = false;
  bool paused = false;

  /// The epoch of the open socket session, if one is open.
  int? session;
  int outstanding = 0;

  /// The Scopes the open session's handshake covered. A removal drops its
  /// Scope, because the acknowledgement belonged to the registration that
  /// went: a registration created after it has never been acknowledged and is
  /// `connecting` until a session subscribes it.
  final acknowledged = <String>{};
}

/// One durable subscription as the application holds it. It is a handle, not
/// the owner of a socket or of the subscription's lifetime.
class Subscription {
  final String scope;
  final int subscriptionId;
  final _Lane _lane;
  final Future<void> Function() _remove;
  final _Load _load;
  SubscriptionInitialization _initialization;

  /// The last committed run this handle has seen; phases never move backwards.
  BootstrapRun? _run;
  final _waiters = <_Waiter>[];

  /// This handle committed its own removal: further removals are a no-op.
  bool _removed = false;

  /// The handle was stopped with its client: its status is readable, its work
  /// is not.
  bool _stopped = false;
  final _sinks = <MultiStreamController<SubscriptionStatus>>[];
  late SubscriptionStatus _snapshot;
  Subscription._(SubscriptionState state, this._lane, this._remove, this._load)
    : scope = state.scope,
      subscriptionId = state.subscriptionId,
      _initialization = state.startingCursor == null
          ? SubscriptionInitialization.pending
          : SubscriptionInitialization.ready {
    _snapshot = _project();
  }
  SubscriptionStatus get status => _snapshot;
  bool get _closed => _removed || _stopped;

  /// The one place a status comes from: what is committed for this
  /// subscription and what its lane is doing.
  SubscriptionStatus _project() => SubscriptionStatus(
    active: !_closed,
    initialization: _initialization,
    connection: _closed
        ? SubscriptionConnection.stopped
        : !_lane.attached || _lane.paused
        ? SubscriptionConnection.offline
        : _lane.session == null
        ? SubscriptionConnection.connecting
        : _lane.outstanding > 0
        ? SubscriptionConnection.catchingUp
        : _lane.acknowledged.contains(scope)
        ? SubscriptionConnection.live
        : SubscriptionConnection.connecting,
    bootstrap: _bootstrap(),
  );

  /// What the stored run projects to; the stored record reports are not part of
  /// the public failure.
  BootstrapStatus _bootstrap() {
    final run = _run;
    if (run == null) {
      return const BootstrapStatus(phase: BootstrapPhase.notRequested);
    }
    return BootstrapStatus(
      phase: run.phase(
        initialized: _initialization == SubscriptionInitialization.ready,
      ),
      error: run.error,
    );
  }

  /// Publish a new snapshot when anything changed.
  void _refresh() {
    final next = _project();
    if (next == _snapshot) return;
    _snapshot = next;
    for (final sink in _sinks.toList()) {
      sink.add(next);
    }
  }

  /// The committed state of this identity; another identity's state is not
  /// this handle's, and a closed handle takes none.
  void _apply(SubscriptionState? state) {
    if (_closed) return;
    if (state == null || state.subscriptionId != subscriptionId) return;
    _initialization = state.startingCursor == null
        ? SubscriptionInitialization.pending
        : SubscriptionInitialization.ready;
    _refresh();
  }

  /// Read what is committed for this identity's load. A handle taken after a
  /// restart names a task that may already be running or finished, and no
  /// further transition has to commit for its status to be true.
  void _observe() {
    final zone = Zone.current;
    unawaited(
      _load.read().then(
        _applyBootstrap,
        onError: (Object error, StackTrace stack) {
          if (!_closed) zone.handleUncaughtError(error, stack);
        },
      ),
    );
  }

  /// One committed transition of this identity's load. The waiters of that run
  /// are settled by it whatever the status already shows, so a re-read that
  /// raced ahead of a lane signal cannot swallow an earlier run's outcome; the
  /// status itself never moves backwards.
  void _applyBootstrap(BootstrapRun run) {
    if (_closed) return;
    if (run.subscriptionId != subscriptionId) return;
    if (run.state == 'complete') {
      _settle(run.run, null);
    } else if (run.state == 'failed') {
      // A failed run always carries its stored failure; the ledger refuses any
      // other pairing.
      final failure =
          run.error ??
          BootstrapError(
            code: 'bootstrap.failed',
            message: 'the bootstrap of $scope failed',
          );
      _settle(run.run, BootstrapFailedException(failure.code, failure.message));
    }
    _supersede(run.run);
    final known = _run;
    if (known != null &&
        (run.run < known.run ||
            (run.run == known.run && run.rank < known.rank))) {
      return;
    }
    _run = run;
    _refresh();
  }

  /// Waiters of a run older than the one just observed. Their run is over and
  /// its outcome is no longer observable, and an older call must never complete
  /// from a newer run, so they are failed rather than left attached forever.
  void _supersede(int run) {
    final stale = _waiters.where((waiter) => waiter.run < run).toList();
    if (stale.isEmpty) return;
    _waiters.removeWhere((waiter) => waiter.run < run);
    for (final waiter in stale) {
      waiter.completer.completeError(BootstrapSupersededException(scope));
    }
  }

  void _settle(int run, Object? error) {
    final settled = _waiters.where((waiter) => waiter.run == run).toList();
    if (settled.isEmpty) return;
    _waiters.removeWhere((waiter) => waiter.run == run);
    for (final waiter in settled) {
      if (error == null) {
        waiter.completer.complete();
      } else {
        waiter.completer.completeError(error);
      }
    }
  }

  /// Prepare this Scope's published history. The registration is submitted when
  /// the call is made, whether or not the returned Future is awaited; the Future
  /// completes only after the completion transaction commits. Calls during one
  /// active run share it, a call after a valid completion completes locally -
  /// offline too - and a call after a terminal failure explicitly retries the
  /// saved run.
  Future<void> bootstrap() {
    if (_closed) return Future.error(const SubscriptionClosedException());
    // Eager: the registration is submitted when the call is made, not when the
    // returned Future is awaited.
    return _register(_load.request());
  }

  Future<void> _register(Future<BootstrapRun> submitted) async {
    final BootstrapRun run;
    try {
      run = await submitted;
    } on Object catch (error) {
      if (_closed) throw _closedError();
      // The removal committed between the command and this handle's close: the
      // engine refused a registration that is gone, and this call is one
      // through a closed subscription however the two raced.
      if (_refusedAsClosed(error)) throw const SubscriptionClosedException();
      rethrow;
    }
    // A closed client or handle takes nothing further, not even the wake: the
    // controller it would go through is closed too.
    if (_closed) throw _closedError();
    // The commit wakes the lanes the way a membership change does; without it
    // the registered run waits for the next commit or reconnection.
    _load.committed();
    final waiter = _Waiter(run.run);
    _waiters.add(waiter);
    _applyBootstrap(run);
    // A transition that committed between the command and this waiter would
    // otherwise be missed: re-read the stored run through the same serialized
    // path and apply it.
    if (_waiters.isNotEmpty) _observe();
    return waiter.completer.future;
  }

  Exception _closedError() => _stopped
      ? const ClientClosedException()
      : const SubscriptionClosedException();

  /// Removed durably through this handle, or stopped with the client.
  void _close({required bool removed}) {
    if (_closed) return;
    if (removed) {
      _removed = true;
    } else {
      _stopped = true;
    }
    // A removal took the epoch's load state with the row; a client close leaves
    // the durable task exactly where it was. Either way this process stops
    // waiting for it.
    final waiting = _waiters.toList();
    _waiters.clear();
    for (final waiter in waiting) {
      waiter.completer.completeError(
        removed
            ? const SubscriptionClosedException()
            : const ClientClosedException(),
      );
    }
    _refresh();
    // A closed handle has no changes left: its streams end after that last
    // snapshot, whether it was unsubscribed or stopped with its client.
    for (final sink in _sinks.toList()) {
      sink.close();
    }
    _sinks.clear();
  }

  /// The current snapshot, then every change. Dart's stream convention
  /// applies: the current snapshot arrives on the microtask after `listen`.
  /// Cancelling removes the observer, not the persistent subscription; a
  /// closed handle delivers its stopped snapshot and completes.
  Stream<SubscriptionStatus> watch() =>
      Stream<SubscriptionStatus>.multi((sink) {
        sink.add(_snapshot);
        if (_closed) {
          sink.close();
          return;
        }
        _sinks.add(sink);
        sink.onCancel = () {
          _sinks.remove(sink);
        };
      });

  /// Commit the local removal of this registration and stop the handle.
  /// Repeating it on a closed handle is a no-op; a handle its client stopped
  /// cannot commit work at all.
  Future<void> unsubscribe() async {
    if (_removed) return;
    if (_stopped) throw const SubscriptionClosedException();
    await _remove();
  }
}

/// The registry: one handle per persistent subscription identity, the lane
/// projection they share, and the committed status they publish.
class Subscriptions {
  final SubscriptionCommands _commands;
  final _handles = <int, Subscription>{};

  /// The client closed: a read still in flight answers to nobody.
  bool _closed = false;
  final _lane = _Lane();
  Subscriptions(this._commands);

  /// Register durable intent and answer with the handle of the identity that
  /// commit belongs to. Concurrent calls run through the same serialized
  /// command path, read the same identity and share one cached handle.
  Future<Subscription> subscribe(String scope) async {
    final state = await _commands.subscribe(scope);
    final existing = _handles[state.subscriptionId];
    if (existing != null) {
      existing._apply(state);
      _commands.committed();
      return existing;
    }
    final handle = Subscription._(
      state,
      _lane,
      () => _removeIdentity(state.scope, state.subscriptionId),
      _Load(
        () => _commands.requestBootstrap(state.scope, state.subscriptionId),
        () => _commands.bootstrapState(state.scope, state.subscriptionId),
        _commands.committed,
      ),
    );
    _handles[state.subscriptionId] = handle;
    // A task of this identity may already be running from before this handle:
    // read what is committed for it, so its status needs no new transition.
    handle._observe();
    // The lane learns of committed membership from Rust; it is only woken here.
    _commands.committed();
    return handle;
  }

  /// Remove whatever registration a Scope name has - the Scope-named form the
  /// generated `channels` facade keeps - and close the handle it had.
  Future<void> unsubscribeScope(String scope) async {
    await _commands.removeScope(scope);
    for (final handle in _handles.values.toList()) {
      if (handle.scope == scope) {
        _handles.remove(handle.subscriptionId);
        handle._close(removed: true);
      }
    }
    _forget(scope);
    _commands.committed();
  }

  Future<void> _removeIdentity(String scope, int subscriptionId) async {
    final removed = await _commands.remove(scope, subscriptionId);
    final handle = _handles.remove(subscriptionId);
    handle?._close(removed: true);
    // Nothing went: another registration is this Scope's current one, and the
    // acknowledgement it may hold is not this handle's to forget.
    if (removed) {
      _forget(scope);
      _commands.committed();
    }
  }

  /// The open session's handshake covered the registration that just went, not
  /// the one a later subscribe creates: forget the Scope, so a recreated
  /// subscription is `connecting` until a session of its own acknowledges it.
  void _forget(String scope) {
    _lane.acknowledged.remove(scope);
    _publish();
  }

  /// The replica was replaced: the ledger this client reads now holds fresh
  /// identities, so every handle names a registration of the file that was
  /// left behind. They close the way an unsubscribed handle does - `watch`
  /// completes and a later `unsubscribe` is a harmless no-op - and the lane
  /// forgets its acknowledgements, because none of them belong to an identity
  /// that still exists.
  void rebuilt() {
    final handles = _handles.values.toList();
    _handles.clear();
    _lane.acknowledged.clear();
    for (final handle in handles) {
      handle._close(removed: true);
    }
  }

  /// The lane is running for this client: until then, and once it closes,
  /// every subscription is offline.
  void attach() {
    _lane.attached = true;
    _lane.paused = false;
    _publish();
  }

  void detach() {
    _lane.attached = false;
    _endSession();
  }

  void signal(DownlinkSignal signal) {
    switch (signal.lane) {
      case 'opened':
        _lane.session = signal.epoch;
        _lane.outstanding = 0;
        _lane.acknowledged.clear();
      case 'ended':
        // Whatever an abandoned session still reports belongs to no lane state.
        if (_lane.session != signal.epoch) return;
        _endSession();
        return;
      case 'paused':
        _lane.paused = true;
        _endSession();
        return;
      case 'resumed':
        _lane.paused = false;
      case 'stopped':
        _lane.attached = false;
        _endSession();
        return;
      case 'requests':
        _lane.outstanding = signal.outstanding!;
      case 'acknowledged':
        _lane.acknowledged.addAll(signal.scopes);
      case 'changed':
        // A commit moved these Scopes: re-read what it committed for them.
        for (final scope in signal.scopes) {
          _reload(scope);
        }
      case 'bootstrap':
        // A committed load transition. Another epoch's run belongs to no handle
        // this registry still holds.
        final run = BootstrapRun.fromRecord(signal.run!);
        _handles[run.subscriptionId]?._applyBootstrap(run);
    }
    _publish();
  }

  void _endSession() {
    _lane.session = null;
    _lane.outstanding = 0;
    _lane.acknowledged.clear();
    _publish();
  }

  void _publish() {
    for (final handle in _handles.values.toList()) {
      handle._refresh();
    }
  }

  /// Read the committed state of a Scope and hand it to the handle it belongs
  /// to.
  void _reload(String scope) {
    final handles = _handles.values
        .where((handle) => handle.scope == scope && !handle._closed)
        .toList();
    if (handles.isEmpty) return;
    final zone = Zone.current;
    unawaited(
      _commands
          .state(scope)
          .then(
            (state) {
              for (final handle in handles) {
                handle._apply(state);
              }
            },
            onError: (Object error, StackTrace stack) {
              if (!_closed) zone.handleUncaughtError(error, stack);
            },
          ),
    );
  }

  /// The client closed: every handle stops and its observers are cancelled; no
  /// subscription is removed.
  void close() {
    _closed = true;
    final handles = _handles.values.toList();
    _handles.clear();
    _lane.attached = false;
    _endSession();
    for (final handle in handles) {
      handle._close(removed: false);
    }
  }
}
