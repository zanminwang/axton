import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:axton/axton.dart';
import 'package:test/test.dart';

void main() {
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
}
