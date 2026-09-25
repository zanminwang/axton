// Subscription handles: identity, the committed status they publish, their
// observers, and what closing one means
// ([#150](https://github.com/zanminwang/axton/issues/150)).
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:axton/axton.dart';
import 'package:test/test.dart';

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
  FakeServer(this.server);
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
        final observer = subscription.watch().listen(seen.add);
        await pumpEventQueue();
        expect(seen.map((s) => s.connection), [SubscriptionConnection.offline]);
        await subscription.unsubscribe();
        await pumpEventQueue();
        expect(seen.map((s) => [s.active, s.connection]), [
          [true, SubscriptionConnection.offline],
          [false, SubscriptionConnection.stopped],
        ]);
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
}
