import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:axton/axton.dart';
import 'package:axton/src/connection.dart' show DownlinkLane;
import 'package:axton/src/live.dart' show ServerSession;
import 'package:test/test.dart';

void main() {
  Future<void> assertSocketClosed({required bool closeConnection}) async {
    final directory = await Directory.systemTemp.createTemp(
      'axton-direct-abort-',
    );
    final server = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
    final entered = Completer<void>();
    final disconnected = Completer<void>();
    Socket? accepted;
    final subscription = server.listen((socket) {
      accepted = socket;
      socket.listen(
        (_) {
          if (!entered.isCompleted) entered.complete();
        },
        onDone: () {
          if (!disconnected.isCompleted) disconnected.complete();
        },
        onError: (Object _) {
          if (!disconnected.isCompleted) disconnected.complete();
        },
      );
    });
    final client = await Client.open(
      path: '${directory.path}/db',
      schema: {
        'enums': [],
        'models': [],
        'actions': [
          {'name': 'Ping', 'version': 1, 'inputs': [], 'outputs': []},
        ],
      },
      libraryPath: Platform.environment['AXTON_LIBRARY']!,
    );
    RuntimeConnection? connection;
    try {
      connection = await client.connect(
        SyncServer(
          url: 'http://127.0.0.1:${server.port}',
          token: () => 'alice',
        ),
        directTimeout: Duration(milliseconds: closeConnection ? 1000 : 100),
      );
      final pending = client.callAction('Ping', 1, {});
      final observed = expectLater(
        pending,
        throwsA(isA<ActionTransportException>()),
      );
      await entered.future.timeout(const Duration(seconds: 1));
      if (closeConnection) await connection.close();
      await observed;
      await disconnected.future.timeout(const Duration(milliseconds: 300));
    } finally {
      await connection?.close();
      await client.close();
      accepted?.destroy();
      await subscription.cancel();
      await server.close();
      await directory.delete(recursive: true);
    }
  }

  test(
    'direct timeout closes its actual HTTP socket',
    () async => assertSocketClosed(closeConnection: false),
  );
  test(
    'direct close closes its actual HTTP socket',
    () async => assertSocketClosed(closeConnection: true),
  );

  test('background pause does not invalidate a direct token wait', () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final token = Completer<String>();
    final received = Completer<String>();
    final served = server.listen((request) async {
      received.complete(request.uri.path);
      await utf8.decoder.bind(request).join();
      request.response.write('ok');
      await request.response.close();
    });
    final session = ServerSession(
      SyncServer(
        url: 'http://127.0.0.1:${server.port}',
        token: () => token.future,
      ),
    );
    final cancellation = Completer<void>();
    try {
      final direct = session.action('{}', cancellation.future);
      session.cancelPush();
      token.complete('alice');
      expect(await direct, 'ok');
      expect(await received.future, '/sync/actions');
    } finally {
      cancellation.complete();
      await served.cancel();
      await server.close(force: true);
    }
  });
  test(
    'cancelling one direct HTTP attempt leaves a sibling request alive',
    () async {
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      final firstSeen = Completer<void>();
      final served = server.listen((request) async {
        final body = await utf8.decoder.bind(request).join();
        if (body == 'first') {
          firstSeen.complete();
          return;
        }
        request.response.write('second-ok');
        await request.response.close();
      });
      final session = ServerSession(
        SyncServer(
          url: 'http://127.0.0.1:${server.port}',
          token: () => 'alice',
        ),
      );
      final cancelFirst = Completer<void>();
      final cancelSecond = Completer<void>();
      try {
        final first = session.action('first', cancelFirst.future);
        final failed = expectLater(first, throwsA(anything));
        await firstSeen.future;
        final second = session.action('second', cancelSecond.future);
        cancelFirst.complete();
        await failed;
        expect(await second, 'second-ok');
      } finally {
        if (!cancelFirst.isCompleted) cancelFirst.complete();
        if (!cancelSecond.isCompleted) cancelSecond.complete();
        await served.cancel();
        await server.close(force: true);
      }
    },
  );
  test('wake while idle decision is in flight is retained', () async {
    final gate = Completer<void>();
    var calls = 0;
    final connection = await RuntimeConnection.start(
      control: (event, now, entropy) async {
        if (event == 'next') {
          if (++calls == 1) await gate.future;
          return {'type': 'idle'};
        }
        return null;
      },
      sync: (_) async {},
      transport: (_, __) async => '',
    );
    await connection.wake();
    gate.complete();
    await Future<void>.delayed(const Duration(milliseconds: 10));
    expect(calls, greaterThanOrEqualTo(2));
    await connection.close();
  });
  // The downlink lane's own run loop, mirroring the push lane above and the
  // TypeScript host's `downlink wake arriving during the idle decision cannot
  // be lost` ([#150](https://github.com/zanminwang/axton/issues/150)).
  test(
    'downlink wake arriving during the idle decision cannot be lost',
    () async {
      final gate = Completer<void>();
      var pumps = 0;
      final events = <String>[];
      final lane = await DownlinkLane.start(
        command: (event) async {
          events.add(event['event'] as String);
          if (event['event'] != 'next') return const [];
          if (++pumps == 1) await gate.future;
          return const [];
        },
        network: ServerSession(
          SyncServer(url: 'http://127.0.0.1:1', token: () => 'secret'),
        ),
        wakePush: () {},
      );
      await lane.wake();
      gate.complete();
      await Future<void>.delayed(const Duration(milliseconds: 20));
      expect(pumps, greaterThanOrEqualTo(2), reason: 'the wake was lost');
      expect(events.take(2), ['start', 'next']);
      await lane.close();
      expect(events, contains('stop'));
    },
  );
  test('close abandons a transport which never resolves', () async {
    final entered = Completer<void>();
    final never = Completer<String>();
    final events = <String>[];
    final connection = await RuntimeConnection.start(
      control: (event, now, entropy) async {
        events.add(event);
        return {'type': 'sync'};
      },
      sync: (request) async {
        entered.complete();
        await request('push', '{}');
      },
      transport: (_, __) => never.future,
    );
    await entered.future;
    await connection.close();
    await Future<void>.delayed(Duration.zero);
    expect(events, contains('stop'));
    expect(events, isNot(contains('success')));
    expect(events, isNot(contains('failure')));
  });
  test('closed controls cannot alter a replacement driver', () async {
    final events = <String>[];
    final connection = await RuntimeConnection.start(
      control: (event, now, entropy) async {
        events.add(event);
        return {'type': 'idle'};
      },
      sync: (_) async {},
      transport: (_, __) async => '',
    );
    await connection.close();
    final ended = events.length;
    await connection.pause();
    await connection.resume();
    await connection.wake();
    await connection.close();
    expect(events.length, ended);
  });
  test('direct attempt times out while carrier ignores cancellation', () async {
    final connection = await RuntimeConnection.start(
      control: (event, now, entropy) async => {'type': 'idle'},
      sync: (_) async {},
      transport: (_, __) => Completer<String>().future,
      directTimeout: const Duration(milliseconds: 15),
    );
    try {
      await expectLater(
        connection.requestAction('same bytes'),
        throwsA(
          isA<ActionTransportException>().having(
            (e) => e.code,
            'code',
            'action.execution_unknown',
          ),
        ),
      );
    } finally {
      await connection.close();
    }
  });
  test(
    'direct authentication retry preserves request and close ends a pending attempt',
    () async {
      final seen = <String>[];
      final connection = await RuntimeConnection.start(
        control: (event, now, entropy) async => {'type': 'idle'},
        sync: (_) async {},
        transport: (kind, body) async {
          seen.add('$kind:$body');
          if (seen.length == 1) throw const AuthenticationExpired();
          return 'ok';
        },
        refreshAuth: () async {},
      );
      expect(await connection.requestAction('frozen'), 'ok');
      expect(seen, ['action:frozen', 'action:frozen']);
      final hanging = RuntimeConnection.start(
        control: (event, now, entropy) async => {'type': 'idle'},
        sync: (_) async {},
        transport: (_, __) => Completer<String>().future,
      );
      final second = await hanging;
      final pending = second.requestAction('late');
      final observed = expectLater(
        pending,
        throwsA(
          isA<ActionTransportException>().having(
            (e) => e.code,
            'code',
            'action.unavailable',
          ),
        ),
      );
      await second.close();
      await observed;
      await connection.close();
    },
  );
  test('direct timeout includes a stalled authentication refresh', () async {
    final connection = await RuntimeConnection.start(
      control: (event, now, entropy) async => {'type': 'idle'},
      sync: (_) async {},
      transport: (_, __) async => throw const AuthenticationExpired(),
      refreshAuth: () => Completer<void>().future,
      directTimeout: const Duration(milliseconds: 15),
    );
    try {
      await expectLater(
        connection.requestAction('frozen'),
        throwsA(
          isA<ActionTransportException>().having(
            (e) => e.code,
            'code',
            'action.execution_unknown',
          ),
        ),
      );
    } finally {
      await connection.close();
    }
  });
  test(
    'direct HTTP wait leaves local work free and late response after close is ignored',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'axton-dart-direct-',
      );
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      final entered = Completer<Map<String, dynamic>>();
      final release = Completer<void>();
      final served = server.listen((request) async {
        if (request.uri.path != '/sync/actions') {
          request.response.statusCode = 404;
          await request.response.close();
          return;
        }
        final body =
            jsonDecode(await utf8.decoder.bind(request).join())
                as Map<String, dynamic>;
        entered.complete(body);
        await release.future;
        request.response.write(
          jsonEncode({
            'completion': {
              'callId': (body['call'] as Map)['callId'],
              'outcome': {'status': 'succeeded', 'result': null},
            },
            'records': [],
          }),
        );
        await request.response.close();
      });
      final client = await Client.open(
        path: '${directory.path}/db',
        schema: {
          'enums': [],
          'models': [],
          'actions': [
            {'name': 'Ping', 'version': 1, 'inputs': [], 'outputs': []},
          ],
        },
        libraryPath: Platform.environment['AXTON_LIBRARY']!,
      );
      RuntimeConnection? connection;
      try {
        connection = await client.connect(
          SyncServer(
            url: 'http://127.0.0.1:${server.port}',
            token: () => 'alice',
          ),
        );
        final pending = client.callAction('Ping', 1, {});
        final observed = expectLater(
          pending,
          throwsA(isA<ActionTransportException>()),
        );
        await entered.future;
        expect((await client.syncState())['pending'], 0);
        expect(await client.transaction((_) async => 42), 42);
        await connection.close();
        await observed;
        release.complete();
        await Future<void>.delayed(const Duration(milliseconds: 10));
        expect((await client.syncState())['pending'], 0);
      } finally {
        if (!release.isCompleted) release.complete();
        await connection?.close();
        await client.close();
        await served.cancel();
        await server.close(force: true);
        await directory.delete(recursive: true);
      }
    },
  );
  test(
    'response ready during close cannot apply behind a local transaction',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'axton-direct-close-race-',
      );
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      final entered = Completer<Map<String, dynamic>>();
      final releaseResponse = Completer<void>();
      final responseSent = Completer<void>();
      final served = server.listen((request) async {
        final body =
            jsonDecode(await utf8.decoder.bind(request).join())
                as Map<String, dynamic>;
        entered.complete(body);
        await releaseResponse.future;
        request.response.write(
          jsonEncode({
            'completion': {
              'callId': (body['call'] as Map)['callId'],
              'outcome': {'status': 'succeeded', 'result': null},
            },
            'records': [],
          }),
        );
        await request.response.close();
        responseSent.complete();
      });
      final client = await Client.open(
        path: '${directory.path}/db',
        schema: {
          'enums': [],
          'models': [],
          'actions': [
            {'name': 'Ping', 'version': 1, 'inputs': [], 'outputs': []},
          ],
        },
        libraryPath: Platform.environment['AXTON_LIBRARY']!,
      );
      final completions = <Map<String, dynamic>>[];
      final observedCompletions = client.actionCompletions.listen(
        completions.add,
      );
      RuntimeConnection? connection;
      final hold = Completer<void>();
      try {
        connection = await client.connect(
          SyncServer(
            url: 'http://127.0.0.1:${server.port}',
            token: () => 'alice',
          ),
        );
        final pending = client.callAction('Ping', 1, {});
        final failed = expectLater(
          pending,
          throwsA(isA<ActionTransportException>()),
        );
        await entered.future;
        final txEntered = Completer<void>();
        final transaction = client.transaction((_) async {
          txEntered.complete();
          await hold.future;
        });
        await txEntered.future;
        releaseResponse.complete();
        await responseSent.future;
        await Future<void>.delayed(const Duration(milliseconds: 10));
        final closing = connection.close();
        hold.complete();
        await transaction;
        await closing;
        await failed;
        expect(completions, isEmpty);
      } finally {
        if (!hold.isCompleted) hold.complete();
        if (!releaseResponse.isCompleted) releaseResponse.complete();
        await connection?.close();
        await observedCompletions.cancel();
        await client.close();
        await served.cancel();
        await server.close(force: true);
        await directory.delete(recursive: true);
      }
    },
  );
  test(
    'Dart Action discard and rebuild streams carry terminal call identities',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'axton-dart-action-discard-',
      );
      final schema =
          jsonDecode(
                await File('../../fixtures/schemas/entry.json').readAsString(),
              )
              as Map<String, dynamic>;
      schema['actions'] = [
        {'name': 'Ping', 'version': 1, 'inputs': [], 'outputs': []},
      ];
      final breaking = jsonDecode(jsonEncode(schema)) as Map<String, dynamic>;
      ((breaking['models'] as List).first['fields'] as List).add({
        'name': 'due',
        'nullable': false,
        'type': {'kind': 'scalar', 'name': 'string'},
      });
      try {
        for (final frozen in [false, true]) {
          final path = '${directory.path}/${frozen ? 'frozen' : 'unsent'}';
          final original = await Client.open(
            path: path,
            schema: schema,
            libraryPath: Platform.environment['AXTON_LIBRARY']!,
          );
          final delivered = <Map<String, dynamic>>[];
          final sub = original.actionCompletions.listen(delivered.add);
          final dropped = await original.submitAction('Ping', 1, {});
          await original.drop(dropped['ordinal'] as int);
          expect(delivered.single['callId'], dropped['callId']);
          expect((delivered.single['outcome'] as Map)['code'], 'dropped');
          final pending = await original.submitAction('Ping', 1, {});
          if (frozen) await original.freeze();
          await sub.cancel();
          await original.close();
          final reopened = await Client.open(
            path: path,
            schema: breaking,
            libraryPath: Platform.environment['AXTON_LIBRARY']!,
          );
          final abandoned = <Map<String, dynamic>>[];
          final rebuildSub = reopened.actionCompletions.listen(abandoned.add);
          try {
            final report = await reopened.rebuild(discardPending: true);
            expect(report['abandonedCalls'], [
              {'callId': pending['callId'], 'frozen': frozen},
            ]);
            expect(abandoned.single['callId'], pending['callId']);
            expect(
              (abandoned.single['outcome'] as Map)['execution'],
              frozen ? 'unknown' : 'rejected',
            );
          } finally {
            await rebuildSub.cancel();
            await reopened.close();
          }
        }
      } finally {
        await directory.delete(recursive: true);
      }
    },
  );

  /// A page the lane abandoned itself is not the application's failure: `pause`
  /// aborts it silently, the worker still hears `failed` so it can clear its
  /// slot, and `resume` fetches again on a cancellation of its own. The
  /// TypeScript twin is `pausing the downlink lane abandons its bootstrap page
  /// without reporting it`
  /// ([#151](https://github.com/zanminwang/axton/issues/151)).
  test(
    'pausing the downlink lane abandons its bootstrap page without reporting it',
    () async {
      const body =
          '{"mode":"bootstrap","channel":"a","models":{},"after":0,"until":7}';
      final reported = <Object>[];
      final events = <Map<String, dynamic>>[];
      final script = <List<dynamic>>[
        [
          {'type': 'request', 'request': 4, 'body': body, 'bootstrap': true},
        ],
        <dynamic>[],
      ];
      final network = _ScriptedSession();
      final lane = await DownlinkLane.start(
        command: (event) async {
          events.add(event);
          if (event['event'] != 'next') return const [];
          return script.isEmpty ? const [] : script.removeAt(0);
        },
        network: network,
        wakePush: () {},
        onError: reported.add,
      );
      await Future<void>.delayed(const Duration(milliseconds: 20));
      expect(network.pulls, 1, reason: 'the page went out');
      await lane.pause();
      await Future<void>.delayed(const Duration(milliseconds: 20));
      expect(
        reported,
        isEmpty,
        reason: 'its own cancellation is not an application failure',
      );
      final failed = events.firstWhere((e) => e['event'] == 'failed');
      expect(failed['request'], 4);
      expect(failed['status'], isNull);
      // Resume fetches again, on a cancellation of its own: the pause does not
      // reach the next page.
      script.add([
        {'type': 'request', 'request': 5, 'body': body, 'bootstrap': true},
      ]);
      await lane.resume();
      await Future<void>.delayed(const Duration(milliseconds: 20));
      expect(network.pulls, 2, reason: 'the resumed page went out');
      expect(events.where((e) => e['event'] == 'failed').length, 1);
      await lane.close();
    },
  );
}

/// A downlink network whose pages answer only when the lane abandons them: the
/// scripted host of the test above.
class _ScriptedSession extends ServerSession {
  _ScriptedSession()
    : super(SyncServer(url: 'http://127.0.0.1:1', token: _token));
  int pulls = 0;
  @override
  Future<String> pull(String body, Future<void> cancellation) {
    pulls++;
    final answer = Completer<String>();
    unawaited(
      cancellation.then((_) {
        if (!answer.isCompleted) {
          answer.completeError(StateError('connection_paused_or_closed'));
        }
      }),
    );
    return answer.future;
  }
}

String _token() => 'secret';
