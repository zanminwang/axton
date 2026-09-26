// Subscription handles: identity, the committed status they publish, their
// observers, and what closing one means
// ([#150](https://github.com/zanminwang/axton/issues/150)).
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:axton/axton.dart';
// The registry itself, for the races only an exact command path can produce.
import 'package:axton/src/subscriptions.dart'
    show BootstrapRun, DownlinkSignal, SubscriptionCommands, Subscriptions;
import 'package:test/test.dart';

/// One stored run, as the native commands answer it.
BootstrapRun stored({int run = 1, String state = 'requested'}) => BootstrapRun(
  scope: 'scope',
  subscriptionId: 1,
  state: state,
  run: run,
  cursor: 0,
);

/// The registry driven straight through its command path: one registration,
/// whose load commands the test supplies
/// ([#151](https://github.com/zanminwang/axton/issues/151)).
Subscriptions registry({
  Future<BootstrapRun> Function()? requestBootstrap,
  Future<BootstrapRun> Function()? read,
}) {
  const state = SubscriptionState(
    scope: 'scope',
    subscriptionId: 1,
    startingCursor: 0,
    cursor: 0,
  );
  return Subscriptions(
    SubscriptionCommands(
      subscribe: (_) async => state,
      state: (_) async => state,
      remove: (_, _) async => true,
      removeScope: (_) async {},
      requestBootstrap: (_, _) => (requestBootstrap ?? () async => stored())(),
      bootstrapState: (_, _) => (read ?? () async => stored())(),
    ),
  );
}

/// The acknowledgement: every subscribed channel at `head`.
String ack(Map sub, int head) => jsonEncode({
  'type': 'subscribed',
  'cursors': {for (final channel in sub['channels'] as List) channel: head},
});
Map<String, Object?> page(String text, int cursor, int stamp) => {
  'cursors': {
    'scope': {'from': cursor, 'to': cursor + 1, 'head': cursor + 1},
  },
  'changes': [
    {
      'model': 'Entry',
      'identity': {'id': 'live'},
      'stamp': stamp,
      'state': {'text': text, 'note': null},
    },
  ],
};

/// One terminal bootstrap page covering the whole requested interval, with the
/// channel head it observed - the barrier completion then waits for
/// ([#151](https://github.com/zanminwang/axton/issues/151)).
Map<String, Object?> loaded(Map body, int head) => {
  'mode': 'bootstrap',
  'channel': body['channel'],
  'from': body['after'],
  'to': body['until'],
  'until': body['until'],
  'head': head,
  'records': <Object>[],
};

Future<void> until(Future<bool> Function() predicate, String what) async {
  final deadline = DateTime.now().add(const Duration(seconds: 5));
  while (DateTime.now().isBefore(deadline)) {
    if (await predicate()) return;
    await Future<void>.delayed(const Duration(milliseconds: 5));
  }
  throw StateError('$what timed out');
}

/// One client on a temporary database, with the fixture schema.
class Fixture {
  final Client client;
  final Directory directory;
  Fixture(this.client, this.directory);
  static Future<Fixture> open() async {
    final directory = await Directory.systemTemp.createTemp(
      'axton-dart-subscriptions-',
    );
    return Fixture(await openClient(directory), directory);
  }

  static Future<Client> openClient(Directory directory) async {
    final schema =
        jsonDecode(
              await File('../../fixtures/schemas/entry.json').readAsString(),
            )
            as Map<String, dynamic>;
    return Client.open(
      path: '${directory.path}/db',
      schema: schema,
      libraryPath: Platform.environment['AXTON_LIBRARY']!,
    );
  }

  Future<void> close() async {
    await client.close();
    await directory.delete(recursive: true);
  }
}

/// A fake server whose handshake acknowledges `head` and whose pull answers
/// once `hold` completes.
class FakeServer {
  final HttpServer server;
  final sockets = <WebSocket>[];
  final handshakes = <Map>[];
  final pulls = <Map>[];
  int head = 0;
  Future<void> hold = Future<void>.value();
  Map<String, Object?> Function(Map pull)? answer;

  /// A bootstrap request is answered by [load] behind its own [loadHold]; a
  /// [load] that answers null is refused with HTTP 400.
  Future<void> loadHold = Future<void>.value();
  Map<String, Object?>? Function(Map request)? load;
  FakeServer(this.server);

  /// The bootstrap requests this server was asked for, in order.
  List<Map> get loads =>
      pulls.where((pull) => pull['mode'] == 'bootstrap').toList();
  SyncServer get config =>
      SyncServer(url: 'http://127.0.0.1:${server.port}', token: () => 'secret');
  static Future<FakeServer> start() async {
    final fake = FakeServer(
      await HttpServer.bind(InternetAddress.loopbackIPv4, 0),
    );
    fake.server.listen((request) async {
      if (request.uri.path == '/sync/pull') {
        final pull = jsonDecode(await utf8.decoder.bind(request).join()) as Map;
        fake.pulls.add(pull);
        if (pull['mode'] == 'bootstrap') {
          await fake.loadHold;
          final page = (fake.load ?? (body) => loaded(body, fake.head))(pull);
          if (page == null) {
            request.response.statusCode = 400;
            request.response.write('the server refuses this interval');
          } else {
            request.response.write(jsonEncode(page));
          }
          await request.response.close();
          return;
        }
        await fake.hold;
        request.response.write(
          jsonEncode(
            fake.answer?.call(pull) ??
                {
                  'cursors': {
                    for (final entry in (pull['cursors'] as Map).entries)
                      entry.key: {
                        'from': entry.value,
                        'to': entry.value,
                        'head': entry.value,
                      },
                  },
                  'changes': <Object>[],
                },
          ),
        );
        await request.response.close();
        return;
      }
      final socket = await WebSocketTransformer.upgrade(request);
      fake.sockets.add(socket);
      socket.listen((message) {
        final sub = jsonDecode(message as String) as Map;
        fake.handshakes.add(sub);
        socket.add(ack(sub, fake.head));
      }, onError: (Object _) {});
    });
    return fake;
  }

  Future<void> close() async {
    for (final socket in sockets) {
      await socket.close();
    }
    await server.close(force: true);
  }
}

void main() {
  test(
    'one handle per subscription identity: repeated calls coalesce',
    () async {
      final fixture = await Fixture.open();
      final client = fixture.client;
      try {
        final first = await client.subscribe('scope');
        final second = await client.subscribe('scope');
        expect(identical(first, second), isTrue);
        expect(
          identical(await client.scopes.subscribe('scope'), first),
          isTrue,
        );
        expect(first.scope, 'scope');
        expect(
          first.status,
          const SubscriptionStatus(
            active: true,
            initialization: SubscriptionInitialization.pending,
            connection: SubscriptionConnection.offline,
          ),
          reason: 'registered offline: durable intent with no boundary',
        );
        final state = await client.syncState();
        expect(state['channels'], ['scope']);
        expect(
          state['cursors'],
          isEmpty,
          reason: 'an uninitialized subscription has no cursor at all',
        );
        final other = await client.subscribe('other');
        expect(identical(other, first), isFalse);
      } finally {
        await fixture.close();
      }
    },
  );

  test(
    'watch starts with the current snapshot and stops when cancelled',
    () async {
      final fixture = await Fixture.open();
      final client = fixture.client;
      try {
        final subscription = await client.subscribe('scope');
        final seen = <SubscriptionStatus>[];
        final done = Completer<void>();
        final observer = subscription.watch().listen(
          seen.add,
          onDone: done.complete,
        );
        await pumpEventQueue();
        expect(seen.map((s) => s.connection), [SubscriptionConnection.offline]);
        await subscription.unsubscribe();
        await pumpEventQueue();
        expect(seen.map((s) => [s.active, s.connection]), [
          [true, SubscriptionConnection.offline],
          [false, SubscriptionConnection.stopped],
        ]);
        await done.future.timeout(
          const Duration(seconds: 1),
          onTimeout: () =>
              throw StateError('the stream of a closed handle ends'),
        );
        await observer.cancel();
        final replacement = await client.subscribe('scope');
        expect(identical(replacement, subscription), isFalse);
        await replacement.unsubscribe();
        await pumpEventQueue();
        expect(
          seen,
          hasLength(2),
          reason: 'a cancelled observer hears nothing',
        );
        // The closed handle still reads its status, and watching it delivers the
        // stopped snapshot once and completes.
        expect(await subscription.watch().toList(), [subscription.status]);
        expect(subscription.status.active, isFalse);
      } finally {
        await fixture.close();
      }
    },
  );

  test(
    'an observer exception is reported after the commit and changes nothing',
    () async {
      final fixture = await Fixture.open();
      final client = fixture.client;
      final network = await FakeServer.start();
      final reported = <Object>[];
      try {
        late Subscription subscription;
        await runZonedGuarded(() async {
          subscription = await client.subscribe('scope');
          subscription.watch().listen(
            (_) => throw StateError('observer failed'),
          );
          await pumpEventQueue();
          final connection = await client.connect(network.config);
          await until(
            () async => (await client.syncState())['cursors']['scope'] == 0,
            'the committed boundary',
          );
          await pumpEventQueue();
          expect(
            reported.length,
            greaterThanOrEqualTo(2),
            reason:
                'the failing observer heard the first snapshot and the commit',
          );
          expect(
            (await client.syncState())['channels'],
            ['scope'],
            reason: 'nothing was rolled back',
          );
          // A failing observer is not a transport failure: the session it fired
          // in is still the one streaming.
          network.sockets.last.add(jsonEncode(page('streamed', 0, 1)));
          await until(
            () async =>
                (await client.read('Entry', {'id': 'live'}))?['text'] ==
                'streamed',
            'the streamed page',
          );
          expect(
            network.handshakes,
            hasLength(1),
            reason: 'no reconnect followed the observer failure',
          );
          expect(
            identical(await client.subscribe('scope'), subscription),
            isTrue,
            reason: 'subscribe answered with the same handle, never threw',
          );
          await connection.close();
        }, (error, _) => reported.add(error));
        expect(
          reported.every((e) => e.toString().contains('observer failed')),
          isTrue,
          reason: '$reported',
        );
      } finally {
        await fixture.close();
        await network.close();
      }
    },
  );

  test('status follows the committed boundary and the lane', () async {
    final fixture = await Fixture.open();
    final client = fixture.client;
    final network = await FakeServer.start();
    try {
      final subscription = await client.subscribe('scope');
      final seen = <SubscriptionConnection>[];
      final observer = subscription.watch().listen(
        (status) => seen.add(status.connection),
      );
      expect(
        subscription.status.initialization,
        SubscriptionInitialization.pending,
      );
      final connection = await client.connect(network.config);
      await until(
        () async =>
            subscription.status.connection == SubscriptionConnection.live &&
            subscription.status.initialization ==
                SubscriptionInitialization.ready,
        'a live subscription with a committed boundary',
      );
      await pumpEventQueue();
      expect(
        seen.contains(SubscriptionConnection.connecting),
        isTrue,
        reason: 'the lane was seen connecting: $seen',
      );
      // The server publishes while the socket is closed: the next handshake
      // acknowledges a head above the committed cursor and one catch-up runs.
      await connection.pause();
      await until(
        () async =>
            subscription.status.connection == SubscriptionConnection.offline,
        'a paused lane',
      );
      network.head = 1;
      final gate = Completer<void>();
      network.hold = gate.future;
      network.answer = (pull) => {
        'cursors': {
          'scope': {'from': pull['cursors']['scope'], 'to': 1, 'head': 1},
        },
        'changes': page('caught up', 0, 1)['changes'],
      };
      await connection.resume();
      await until(
        () async =>
            subscription.status.connection == SubscriptionConnection.catchingUp,
        'a catching-up subscription',
      );
      expect(
        subscription.status.initialization,
        SubscriptionInitialization.ready,
        reason: 'the boundary stays committed while catching up',
      );
      gate.complete();
      await until(
        () async =>
            (await client.read('Entry', {'id': 'live'}))?['text'] ==
            'caught up',
        'the catch-up page',
      );
      await until(
        () async =>
            subscription.status.connection == SubscriptionConnection.live,
        'live again after the catch-up',
      );
      expect(
        network.pulls.map((p) => p['cursors']),
        [
          {'scope': 0},
        ],
        reason: 'one pull, from the committed cursor',
      );
      await connection.close();
      expect(
        subscription.status,
        const SubscriptionStatus(
          active: true,
          initialization: SubscriptionInitialization.ready,
          connection: SubscriptionConnection.offline,
        ),
      );
      await observer.cancel();
    } finally {
      await fixture.close();
      await network.close();
    }
  });

  test(
    'a recreated subscription is connecting until its own handshake acknowledges it',
    () async {
      final fixture = await Fixture.open();
      final client = fixture.client;
      final network = await FakeServer.start();
      try {
        final first = await client.subscribe('scope');
        final connection = await client.connect(network.config);
        await until(
          () async =>
              first.status.connection == SubscriptionConnection.live &&
              first.status.initialization == SubscriptionInitialization.ready,
          'a live subscription',
        );
        await first.unsubscribe();
        final second = await client.subscribe('scope');
        expect(identical(second, first), isFalse);
        expect(
          second.status,
          const SubscriptionStatus(
            active: true,
            initialization: SubscriptionInitialization.pending,
            connection: SubscriptionConnection.connecting,
          ),
          reason:
              'the open session never subscribed this registration: it is not live',
        );
        // The worker replaces the socket for the new membership; that handshake
        // is this subscription's own.
        await until(() async => network.handshakes.length >= 2, 'a new socket');
        await until(
          () async =>
              second.status.connection == SubscriptionConnection.live &&
              second.status.initialization == SubscriptionInitialization.ready,
          'the new subscription goes live on its own acknowledgement',
        );
        expect(first.status.connection, SubscriptionConnection.stopped);
        await connection.close();
      } finally {
        await fixture.close();
        await network.close();
      }
    },
  );

  // A replica rebuild carries the Scope names over with fresh identities, so a
  // handle from before it names a registration that no longer exists: the one
  // public path where an identity-fenced removal answers "nothing went" while
  // the Scope has a live registration.
  test(
    'a stale handle from before a rebuild cannot disturb the subscription that replaced it',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'axton-dart-subscriptions-rebuild-',
      );
      final schema =
          jsonDecode(
                await File('../../fixtures/schemas/entry.json').readAsString(),
              )
              as Map<String, dynamic>;
      final breaking = jsonDecode(jsonEncode(schema)) as Map<String, dynamic>;
      ((breaking['models'] as List).first as Map)['fields'].add({
        'name': 'due',
        'nullable': false,
        'type': {'kind': 'scalar', 'name': 'string'},
      });
      final network = await FakeServer.start();
      Client? client;
      try {
        client = await Client.open(
          path: '${directory.path}/db',
          schema: schema,
          libraryPath: Platform.environment['AXTON_LIBRARY']!,
        );
        await client.subscribe('scope');
        // Unsent work keeps the incompatible file open, so the rebuild happens
        // with this client - and its handle - already alive.
        await client.mutate({
          'name': 'Create',
          'operations': [
            {
              'model': 'Entry',
              'op': 'create',
              'identity': {'id': 'e'},
              'values': {'text': 'A', 'note': null},
            },
          ],
        });
        await client.close();
        client = await Client.open(
          path: '${directory.path}/db',
          schema: breaking,
          libraryPath: Platform.environment['AXTON_LIBRARY']!,
        );
        final stale = await client.subscribe('scope');
        final seen = <SubscriptionStatus>[];
        var completed = false;
        final watching = stale.watch().listen(
          seen.add,
          onDone: () => completed = true,
        );
        await client.rebuild(discardPending: true);
        const invalidated = SubscriptionStatus(
          active: false,
          initialization: SubscriptionInitialization.pending,
          connection: SubscriptionConnection.stopped,
        );
        expect(
          stale.status,
          invalidated,
          reason:
              'the rebuild invalidated every handle of the replica it replaced',
        );
        await until(
          () async => completed,
          "the stale handle's observers are completed",
        );
        expect(
          seen.last,
          invalidated,
          reason: 'the observer was told, and the handle has no changes left',
        );
        await watching.cancel();
        final current = await client.subscribe('scope');
        expect(identical(current, stale), isFalse);
        final connection = await client.connect(network.config);
        await until(
          () async =>
              current.status.connection == SubscriptionConnection.live &&
              current.status.initialization == SubscriptionInitialization.ready,
          'the carried subscription goes live',
        );
        expect(
          stale.status,
          invalidated,
          reason:
              'the new session acknowledges the Scope name, not the stale handle',
        );
        // The old handle removes nothing: the Scope's current registration is
        // another identity, whose acknowledgement is not this handle's to
        // forget.
        await stale.unsubscribe();
        expect(
          (await client.syncState())['channels'],
          ['scope'],
          reason: 'the current registration stands',
        );
        expect(
          current.status,
          const SubscriptionStatus(
            active: true,
            initialization: SubscriptionInitialization.ready,
            connection: SubscriptionConnection.live,
          ),
          reason: 'a removal that removed nothing changes no status',
        );
        expect(stale.status.connection, SubscriptionConnection.stopped);
        await connection.close();
      } finally {
        await client?.close();
        await network.close();
        await directory.delete(recursive: true);
      }
    },
  );

  test(
    'unsubscribe removes one registration; an old handle cannot remove its replacement',
    () async {
      final fixture = await Fixture.open();
      final client = fixture.client;
      try {
        final first = await client.subscribe('scope');
        await first.unsubscribe();
        expect(
          first.status,
          const SubscriptionStatus(
            active: false,
            initialization: SubscriptionInitialization.pending,
            connection: SubscriptionConnection.stopped,
          ),
        );
        expect((await client.syncState())['channels'], isEmpty);
        await first.unsubscribe();
        expect(
          (await client.syncState())['channels'],
          isEmpty,
          reason: 'repeating it on a closed handle is a no-op',
        );
        final second = await client.subscribe('scope');
        expect(identical(second, first), isFalse);
        await first.unsubscribe();
        expect(
          (await client.syncState())['channels'],
          ['scope'],
          reason:
              'an old handle must not delete the subscription that replaced it',
        );
        expect(second.status.active, isTrue);
        // The Scope-named form removes whatever is registered and closes its
        // handle.
        await client.unsubscribe('scope');
        expect((await client.syncState())['channels'], isEmpty);
        expect(second.status.connection, SubscriptionConnection.stopped);
      } finally {
        await fixture.close();
      }
    },
  );

  test(
    'closing the client stops handles and deletes nothing; work through them fails closed',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'axton-dart-subscriptions-close-',
      );
      try {
        final client = await Fixture.openClient(directory);
        final subscription = await client.subscribe('scope');
        final seen = <SubscriptionStatus>[];
        final observer = subscription.watch().listen(seen.add);
        await pumpEventQueue();
        await client.close();
        await pumpEventQueue();
        expect(
          subscription.status,
          const SubscriptionStatus(
            active: false,
            initialization: SubscriptionInitialization.pending,
            connection: SubscriptionConnection.stopped,
          ),
        );
        expect(
          seen.map((s) => s.connection),
          [SubscriptionConnection.offline, SubscriptionConnection.stopped],
          reason: 'observers hear the stop, then the stream completes',
        );
        await expectLater(
          subscription.unsubscribe(),
          throwsA(isA<SubscriptionClosedException>()),
        );
        await observer.cancel();
        final reopened = await Fixture.openClient(directory);
        try {
          expect(
            (await reopened.syncState())['channels'],
            ['scope'],
            reason: 'closing the client deleted nothing',
          );
          expect((await reopened.subscribe('scope')).status.active, isTrue);
        } finally {
          await reopened.close();
        }
      } finally {
        await directory.delete(recursive: true);
      }
    },
  );

  // Whole-Scope bootstrap through the handle
  // ([#151](https://github.com/zanminwang/axton/issues/151)): registration is
  // eager and local, completion is a committed transition, and the status says
  // which of the two the run is waiting for.
  test(
    'bootstrap is submitted eagerly, concurrent calls share one run, and the barrier completes it',
    () async {
      final fixture = await Fixture.open();
      final client = fixture.client;
      final network = await FakeServer.start();
      try {
        final subscription = await client.subscribe('scope');
        final phases = <BootstrapPhase>[];
        final observer = subscription.watch().listen((status) {
          if (phases.isEmpty || phases.last != status.bootstrap.phase) {
            phases.add(status.bootstrap.phase);
          }
        });
        expect(
          subscription.status.bootstrap,
          const BootstrapStatus(phase: BootstrapPhase.notRequested),
          reason: 'a registration asks for no load of its own',
        );
        final connection = await client.connect(network.config);
        await until(
          () async =>
              subscription.status.initialization ==
              SubscriptionInitialization.ready,
          'the committed boundary',
        );
        // The page is held: the calls submit their registration when they are
        // made, so the task runs and the status moves with nobody awaiting the
        // Futures.
        final gate = Completer<void>();
        network.loadHold = gate.future;
        network.load = (body) => loaded(body, 3);
        final first = subscription.bootstrap();
        final second = subscription.bootstrap();
        var settled = false;
        final both = Future.wait([first, second]).then((_) => settled = true);
        await until(
          () async =>
              subscription.status.bootstrap.phase == BootstrapPhase.loading,
          'a registered load',
        );
        await until(() async => network.loads.length == 1, 'the one page');
        expect(
          settled,
          isFalse,
          reason: 'no call completed before the completion committed',
        );
        gate.complete();
        // The terminal page fixed the barrier at the head it saw, which delivery
        // has not reached: the run waits for the stream, not for another page.
        await until(
          () async =>
              subscription.status.bootstrap.phase == BootstrapPhase.catchingUp,
          'the fixed barrier',
        );
        expect(
          settled,
          isFalse,
          reason:
              'a barrier delivery has not reached does not complete the run',
        );
        expect(
          network.loads,
          hasLength(1),
          reason: 'two concurrent calls registered one task',
        );
        network.sockets.last.add(
          jsonEncode({
            'cursors': {
              'scope': {'from': 0, 'to': 3, 'head': 3},
            },
            'changes': [
              {
                'model': 'Entry',
                'identity': {'id': 'live'},
                'stamp': 3,
                'state': {'text': 'delivered', 'note': null},
              },
            ],
          }),
        );
        await both;
        expect(
          subscription.status.bootstrap,
          const BootstrapStatus(phase: BootstrapPhase.complete),
        );
        await pumpEventQueue();
        expect(phases, [
          BootstrapPhase.notRequested,
          BootstrapPhase.loading,
          BootstrapPhase.catchingUp,
          BootstrapPhase.complete,
        ], reason: 'the observed transitions, in the order they committed');
        expect(
          network.loads,
          hasLength(1),
          reason: 'completion asked for no further page',
        );
        await observer.cancel();
        await connection.close();
      } finally {
        await fixture.close();
        await network.close();
      }
    },
  );

  test(
    'a load registered before initialization waits for the boundary, and a closing client rejects its waiters',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'axton-dart-bootstrap-waiting-',
      );
      final client = await Fixture.openClient(directory);
      try {
        final subscription = await client.subscribe('scope');
        final outcome = Future.wait([
          subscription.bootstrap(),
          subscription.bootstrap(),
        ]).then((_) => 'resolved', onError: (Object error) => '$error');
        await until(
          () async =>
              subscription.status.bootstrap.phase ==
              BootstrapPhase.waitingForInitialization,
          'a registered load with no boundary yet',
        );
        expect(
          subscription.status.connection,
          SubscriptionConnection.offline,
          reason: 'waiting for connectivity is not failure',
        );
        // Closing the client is not a failure of the durable task: it rejects
        // the waiters of this process and removes nothing.
        await client.close();
        expect(await outcome, 'client_closed');
        await expectLater(
          subscription.bootstrap(),
          throwsA(isA<SubscriptionClosedException>()),
          reason: 'a stopped handle starts nothing',
        );
      } finally {
        await client.close();
        await directory.delete(recursive: true);
      }
    },
  );

  test(
    'a completed bootstrap resolves offline, and an interrupted one resumes on the next client',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'axton-dart-bootstrap-restart-',
      );
      final network = await FakeServer.start();
      var client = await Fixture.openClient(directory);
      try {
        var subscription = await client.subscribe('scope');
        var connection = await client.connect(network.config);
        await until(
          () async =>
              subscription.status.initialization ==
              SubscriptionInitialization.ready,
          'the committed boundary',
        );
        final gate = Completer<void>();
        network.loadHold = gate.future;
        final interrupted = subscription.bootstrap().then(
          (_) => 'resolved',
          onError: (Object error) => '$error',
        );
        await until(() async => network.loads.length == 1, 'the page');
        await connection.close();
        await client.close();
        expect(
          await interrupted,
          'client_closed',
          reason: 'the waiters of this process were rejected',
        );
        gate.complete();
        // A reopened client resumes the same run from its committed progress,
        // with no new call at all.
        client = await Fixture.openClient(directory);
        subscription = await client.subscribe('scope');
        final observed = <BootstrapPhase>[];
        final observer = subscription.watch().listen((status) {
          if (observed.isEmpty || observed.last != status.bootstrap.phase) {
            observed.add(status.bootstrap.phase);
          }
        });
        connection = await client.connect(network.config);
        await until(
          () async =>
              subscription.status.bootstrap.phase == BootstrapPhase.complete,
          'the resumed run completes with no new call',
        );
        expect(
          network.loads.map((load) => [load['after'], load['until']]),
          [
            [0, 0],
            [0, 0],
          ],
          reason:
              'the resumed run asked for the same interval from the same '
              'progress, and registered no second run',
        );
        expect(
          observed,
          contains(BootstrapPhase.loading),
          reason: 'the resumed run was observable while it ran',
        );
        await observer.cancel();
        await connection.close();
        await client.close();
        // Completion is durable and local: a client with no network at all
        // completes the call from what was committed.
        client = await Fixture.openClient(directory);
        subscription = await client.subscribe('scope');
        await subscription.bootstrap();
        expect(
          subscription.status.bootstrap,
          const BootstrapStatus(phase: BootstrapPhase.complete),
        );
        expect(
          subscription.status.connection,
          SubscriptionConnection.offline,
          reason: 'no transport was needed',
        );
        expect(
          network.loads,
          hasLength(2),
          reason: 'a completed run asks for nothing more',
        );
      } finally {
        await client.close();
        await network.close();
        await directory.delete(recursive: true);
      }
    },
  );

  test(
    'unsubscribing rejects that handle every load it was waiting for',
    () async {
      final fixture = await Fixture.open();
      final client = fixture.client;
      final network = await FakeServer.start();
      try {
        final subscription = await client.subscribe('scope');
        final connection = await client.connect(network.config);
        await until(
          () async =>
              subscription.status.initialization ==
              SubscriptionInitialization.ready,
          'the committed boundary',
        );
        final gate = Completer<void>();
        network.loadHold = gate.future;
        final pending = subscription.bootstrap().then(
          (_) => 'resolved',
          onError: (Object error) => '$error',
        );
        await until(
          () async =>
              subscription.status.bootstrap.phase == BootstrapPhase.loading,
          'a registered load',
        );
        // The row and its load state go together: the epoch's task is gone, so
        // the waiters of that handle cannot be kept.
        await subscription.unsubscribe();
        expect(await pending, 'subscription.closed');
        await expectLater(
          subscription.bootstrap(),
          throwsA(isA<SubscriptionClosedException>()),
        );
        expect(subscription.status.active, isFalse);
        gate.complete();
        await connection.close();
      } finally {
        await fixture.close();
        await network.close();
      }
    },
  );

  test(
    'a failed run stays failed for the calls it belongs to; an explicit retry is another run',
    () async {
      final fixture = await Fixture.open();
      final client = fixture.client;
      final network = await FakeServer.start();
      try {
        final subscription = await client.subscribe('scope');
        final connection = await client.connect(
          network.config,
          onError: (_) {},
        );
        await until(
          () async =>
              subscription.status.initialization ==
              SubscriptionInitialization.ready,
          'the committed boundary',
        );
        network.load = (_) => null;
        final failure = subscription.bootstrap().then<Object?>(
          (_) => null,
          onError: (Object error) => error,
        );
        await until(
          () async =>
              subscription.status.bootstrap.phase == BootstrapPhase.failed,
          'the refused page fails the run',
        );
        final stored = subscription.status.bootstrap.error;
        expect(stored?.code, 'bootstrap.request_rejected');
        expect(stored?.message, contains('refused with HTTP 400'));
        final rejected = await failure;
        expect(rejected, isA<BootstrapFailedException>());
        expect((rejected as BootstrapFailedException).code, stored?.code);
        expect(rejected.message, stored?.message);
        // The retry is a new run, and it cannot turn the call that failed into a
        // success.
        network.load = (body) => loaded(body, network.head);
        await subscription.bootstrap();
        expect(
          subscription.status.bootstrap,
          const BootstrapStatus(phase: BootstrapPhase.complete),
        );
        expect(
          await failure,
          same(rejected),
          reason: 'the earlier call stayed failed',
        );
        expect(network.loads, hasLength(2), reason: 'one page per run');
        await connection.close();
      } finally {
        await fixture.close();
        await network.close();
      }
    },
  );

  test(
    'a registration the engine refuses as closed rejects with subscription.closed',
    () async {
      // The removal committed between this command and the handle's own close,
      // so the engine - not the handle - is what knows it is gone.
      final refusal = StateError(
        'subscription.closed: subscription 1 for scope is closed; '
        'it has no bootstrap state',
      );
      final closed = registry(requestBootstrap: () async => throw refusal);
      final subscription = await closed.subscribe('scope');
      await expectLater(
        subscription.bootstrap(),
        throwsA(isA<SubscriptionClosedException>()),
        reason: "the engine's text must not reach the caller",
      );
      // An unrelated engine failure is still the caller's to see, unchanged.
      final other = StateError('the database is locked');
      final locked = registry(requestBootstrap: () async => throw other);
      final handle = await locked.subscribe('scope');
      await expectLater(handle.bootstrap(), throwsA(same(other)));
    },
  );

  test(
    'a handle closed while its registration commits settles as closed',
    () async {
      final gate = Completer<void>();
      final closing = registry(
        requestBootstrap: () async {
          await gate.future;
          return stored();
        },
      );
      final subscription = await closing.subscribe('scope');
      final pending = subscription.bootstrap().then<Object?>(
        (_) => null,
        onError: (Object error) => error,
      );
      closing.close();
      gate.complete();
      expect(await pending, isA<ClientClosedException>());
    },
  );

  test(
    'a waiter whose run was superseded is rejected, never completed by the newer one',
    () async {
      var answer = stored();
      final retried = registry(
        requestBootstrap: () async => answer,
        read: () async => answer,
      );
      final subscription = await retried.subscribe('scope');
      final first = subscription.bootstrap().then<Object?>(
        (_) => null,
        onError: (Object error) => error,
      );
      await until(
        () async =>
            subscription.status.bootstrap.phase == BootstrapPhase.loading,
        'the registered run',
      );
      // A retry started run 2, so run 1's outcome can no longer be observed:
      // the call it belongs to never completes from another run.
      answer = stored(run: 2, state: 'loading');
      final second = subscription.bootstrap().then<Object?>(
        (_) => null,
        onError: (Object error) => error,
      );
      expect(
        await first,
        isA<BootstrapSupersededException>(),
        reason: 'run 1 must not complete from run 2',
      );
      // The newest run still settles the call that belongs to it.
      var settled = false;
      unawaited(second.then((_) => settled = true));
      await until(() async {
        retried.signal(
          DownlinkSignal.bootstrap({
            'scope': 'scope',
            'subscriptionId': 1,
            'state': 'complete',
            'run': 2,
            'cursor': 0,
            'barrier': null,
            'error': null,
          }),
        );
        return settled;
      }, 'run 2 settles the call that belongs to it');
      expect(await second, isNull);
      expect(
        subscription.status.bootstrap,
        const BootstrapStatus(phase: BootstrapPhase.complete),
      );
    },
  );

  test(
    'unsubscribing a Scope while a bootstrap is submitted rejects it as closed',
    () async {
      final fixture = await Fixture.open();
      try {
        final subscription = await fixture.client.subscribe('scope');
        // The removal and the registration are submitted in that order on the
        // one serialized command path: whether the handle or the engine sees
        // the closed registration first, the caller gets the same failure.
        final removed = fixture.client.unsubscribe('scope');
        final rejected = subscription.bootstrap().then<Object?>(
          (_) => null,
          onError: (Object error) => error,
        );
        await removed;
        expect(await rejected, isA<SubscriptionClosedException>());
        expect(subscription.status.active, isFalse);
      } finally {
        await fixture.close();
      }
    },
  );
}
