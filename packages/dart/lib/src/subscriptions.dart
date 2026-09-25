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

/// One immutable subscription status snapshot.
class SubscriptionStatus {
  /// Whether this handle still names a live registration.
  final bool active;
  final SubscriptionInitialization initialization;
  final SubscriptionConnection connection;
  const SubscriptionStatus({
    required this.active,
    required this.initialization,
    required this.connection,
  });
  @override
  bool operator ==(Object other) =>
      other is SubscriptionStatus &&
      other.active == active &&
      other.initialization == initialization &&
      other.connection == connection;
  @override
  int get hashCode => Object.hash(active, initialization, connection);
  @override
  String toString() =>
      'SubscriptionStatus(active: $active, '
      'initialization: ${initialization.name}, '
      'connection: ${connection.name})';
}

/// Work attempted through a handle that is closed: unsubscribed, or stopped
/// with its client.
class SubscriptionClosedException implements Exception {
  final String code = 'subscription.closed';
  const SubscriptionClosedException();
  @override
  String toString() => 'subscription.closed';
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
  const DownlinkSignal._(
    this.lane, {
    this.epoch,
    this.outstanding,
    this.scopes = const [],
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

  /// A committed membership change: wake the lanes, as every commit does.
  final void Function() committed;
  const SubscriptionCommands({
    required this.subscribe,
    required this.state,
    required this.remove,
    required this.removeScope,
    required this.committed,
  });
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
  SubscriptionInitialization _initialization;

  /// This handle committed its own removal: further removals are a no-op.
  bool _removed = false;

  /// The handle was stopped with its client: its status is readable, its work
  /// is not.
  bool _stopped = false;
  final _sinks = <MultiStreamController<SubscriptionStatus>>[];
  late SubscriptionStatus _snapshot;
  Subscription._(SubscriptionState state, this._lane, this._remove)
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
  );

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

  /// Removed durably through this handle, or stopped with the client.
  void _close({required bool removed}) {
    if (_closed) return;
    if (removed) {
      _removed = true;
    } else {
      _stopped = true;
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
    );
    _handles[state.subscriptionId] = handle;
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
