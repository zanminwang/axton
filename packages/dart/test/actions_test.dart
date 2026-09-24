import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:axton/axton.dart';
import 'package:axton/src/actions.dart' show ActionObservers, ActionWeakState;
import 'package:test/test.dart';

void main() {
  test('weak routes sweep without retaining an abandoned handle', () {
    final refs = <_TestWeak>[];
    final observers = ActionObservers(
      weak: (state) {
        final ref = _TestWeak(state);
        refs.add(ref);
        return ref;
      },
    );
    observers.register<void>('gone', (_) {});
    refs.single.value = null;
    expect(observers.routingCount, 0);
    observers.complete({
      'callId': 'gone',
      'outcome': {'status': 'succeeded', 'result': null},
    });
  });

  test('active wait survives loss of its weak routing target', () async {
    final refs = <_TestWeak>[];
    final observers = ActionObservers(
      weak: (state) {
        final ref = _TestWeak(state);
        refs.add(ref);
        return ref;
      },
    );
    final call = observers.register<String>('held', (value) => value as String);
    final waiting = call.wait();
    refs.single.value = null;
    observers.complete({
      'callId': 'held',
      'outcome': {'status': 'succeeded', 'result': 'ready'},
    });
    final result =
        await waiting.timeout(const Duration(milliseconds: 100))
            as ActionSuccess<String>;
    expect(result.result, 'ready');
  });

  late Directory directory;
  late Client client;
  setUp(() async {
    directory = await Directory.systemTemp.createTemp('axton-actions-');
    client = await Client.open(
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
  });
  tearDown(() async {
    await client.close();
    await directory.delete(recursive: true);
  });

  test('real native acknowledgement settles a cached typed handle', () async {
    final call = await client.invokeAction<void>('Ping', 1, {}, (_) {});
    expect(call.status, ActionStatus.pending);
    final frozen = await client.freeze();
    expect(frozen, isNotNull);
    final request = jsonDecode(frozen!) as Map<String, dynamic>;
    final mutation = (request['mutations'] as List).single as Map;
    final receipt = {
      'clientId': request['clientId'],
      'batchSequence': request['batchSequence'],
      'rejections': [],
      'completions': [
        {
          'callId': mutation['callId'],
          'outcome': {'status': 'succeeded', 'result': null},
        },
      ],
      'records': [],
    };
    final waiting = call.wait();
    await client.acknowledge(request['batchSequence'] as int, receipt);
    expect(await waiting, isA<ActionSuccess<void>>());
    expect(identical(await waiting, await call.wait()), isTrue);
    expect(call.status, ActionStatus.succeeded);
  });

  test('close settles a handle even without a prior wait', () async {
    final call = await client.invokeAction<void>('Ping', 1, {}, (_) {});
    await client.close();
    final outcome = await call.wait();
    expect(outcome, isA<ActionFailure<void>>());
    expect((outcome as ActionFailure<void>).error.code, 'client.closed');
    expect(call.status, ActionStatus.failed);
  });

  test('close racing a queued submit returns a failed handle', () async {
    final entered = Completer<void>();
    final release = Completer<void>();
    final blocker = client.transaction((_) async {
      entered.complete();
      await release.future;
    });
    await entered.future;
    final submitting = client.invokeAction<void>('Ping', 1, {}, (_) {});
    final closing = client.close();
    release.complete();
    await blocker;
    final call = await submitting.timeout(const Duration(seconds: 1));
    final failure = await call.wait() as ActionFailure<void>;
    expect(failure.error.code, 'client.closed');
    await closing;
  });

  test('drop settles a pending handle through native completion', () async {
    final call = await client.invokeAction<void>('Ping', 1, {}, (_) {});
    await client.drop(1);
    final outcome = await call.wait() as ActionFailure<void>;
    expect(outcome.error.code, 'dropped');
  });

  test(
    'typed actions and standalone direct reject inside a transaction',
    () async {
      await client.transaction((tx) async {
        for (final pending in <Future<dynamic>>[
          client.invokeAction<void>('Ping', 1, {}, (_) {}),
          client.invokeDirectAction<void>('Ping', 1, {}, (_) {}),
        ]) {
          await expectLater(
            pending.timeout(const Duration(milliseconds: 300)),
            throwsA(
              isA<ActionError>()
                  .having((e) => e.code, 'code', 'transaction_active')
                  .having((e) => e.execution, 'execution', 'rejected'),
            ),
          );
        }
        await expectLater(
          client
              .direct({
                'model': 'Entry',
                'op': 'delete',
                'identity': {'id': 'e'},
              })
              .timeout(const Duration(milliseconds: 300)),
          throwsA(
            isA<StateError>().having(
              (e) => e.message,
              'message',
              'transaction_active',
            ),
          ),
        );
      });
    },
  );

  test(
    'invalid result decoder settles the handle as observation failure',
    () async {
      final call = await client.invokeAction<String>('Ping', 1, {}, (_) {
        throw const FormatException('bad result');
      });
      final request = jsonDecode((await client.freeze())!) as Map;
      final mutation = (request['mutations'] as List).single as Map;
      await client.acknowledge(request['batchSequence'] as int, {
        'clientId': request['clientId'],
        'batchSequence': request['batchSequence'],
        'rejections': [],
        'completions': [
          {
            'callId': mutation['callId'],
            'outcome': {'status': 'succeeded', 'result': null},
          },
        ],
        'records': [],
      });
      final outcome = await call.wait() as ActionFailure<String>;
      expect(outcome.error.code, 'action.observation_failed');
      expect(outcome.error.cause, isA<FormatException>());
      expect(call.status, ActionStatus.failed);
    },
  );

  test(
    'direct action returns decoded result and maps unavailable transport',
    () async {
      await expectLater(
        client.invokeDirectAction<void>('Ping', 1, {}, (_) {}),
        throwsA(
          isA<ActionError>().having(
            (e) => e.code,
            'code',
            'action.unavailable',
          ),
        ),
      );
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      var succeed = true;
      final sub = server.listen((request) async {
        if (request.uri.path != '/sync/actions') {
          request.response.statusCode = 404;
          await request.response.close();
          return;
        }
        final body = jsonDecode(await utf8.decoder.bind(request).join()) as Map;
        request.response.write(
          jsonEncode({
            'completion': {
              'callId': (body['call'] as Map)['callId'],
              'outcome': succeed
                  ? {'status': 'succeeded', 'result': null}
                  : {
                      'status': 'failed',
                      'code': 'handler.failed',
                      'execution': 'rejected',
                    },
            },
            'records': [],
          }),
        );
        await request.response.close();
      });
      final connection = await client.connect(
        SyncServer(
          url: 'http://127.0.0.1:${server.port}',
          token: () => 'alice',
        ),
      );
      try {
        expect(
          await client.invokeDirectAction<String>(
            'Ping',
            1,
            {},
            (_) => 'decoded',
          ),
          'decoded',
        );
        await expectLater(
          client.invokeDirectAction<String>('Ping', 1, {}, (_) {
            throw const FormatException('bad direct result');
          }),
          throwsA(
            isA<ActionError>()
                .having((e) => e.code, 'code', 'action.observation_failed')
                .having((e) => e.cause, 'cause', isA<FormatException>()),
          ),
        );
        succeed = false;
        await expectLater(
          client.invokeDirectAction<void>('Ping', 1, {}, (_) {}),
          throwsA(
            isA<ActionError>()
                .having((e) => e.code, 'code', 'handler.failed')
                .having((e) => e.execution, 'execution', 'rejected'),
          ),
        );
      } finally {
        await connection.close();
        await sub.cancel();
        await server.close(force: true);
      }
    },
  );

  test('connected native push pump resolves a live typed handle', () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    var executions = 0;
    final served = server.listen((request) async {
      if (request.uri.path != '/sync/mutations') {
        request.response.statusCode = 404;
        await request.response.close();
        return;
      }
      final body = jsonDecode(await utf8.decoder.bind(request).join()) as Map;
      final mutation = (body['mutations'] as List).single as Map;
      executions++;
      request.response.write(
        jsonEncode({
          'clientId': body['clientId'],
          'batchSequence': body['batchSequence'],
          'rejections': [],
          'completions': [
            {
              'callId': mutation['callId'],
              'outcome': {'status': 'succeeded', 'result': null},
            },
          ],
          'records': [],
        }),
      );
      await request.response.close();
    });
    final connection = await client.connect(
      SyncServer(url: 'http://127.0.0.1:${server.port}', token: () => 'alice'),
    );
    try {
      final call = await client.invokeAction<void>('Ping', 1, {}, (_) {});
      final deadline = DateTime.now().add(const Duration(seconds: 2));
      while (call.status == ActionStatus.pending &&
          DateTime.now().isBefore(deadline)) {
        await Future<void>.delayed(const Duration(milliseconds: 5));
      }
      expect(
        call.status,
        ActionStatus.succeeded,
        reason: 'the pump runs without an active wait',
      );
      expect(
        await call.wait().timeout(const Duration(seconds: 2)),
        isA<ActionSuccess<void>>(),
      );
      expect(executions, 1);
      expect((await client.syncState())['pending'], 0);
    } finally {
      await connection.close();
      await served.cancel();
      await server.close(force: true);
    }
  });

  test('standalone direct write commits locally and wakes watch', () async {
    final schema =
        jsonDecode(
              await File('../../fixtures/schemas/entry.json').readAsString(),
            )
            as Map<String, dynamic>;
    final local = await Client.open(
      path: '${directory.path}/local',
      schema: schema,
      libraryPath: Platform.environment['AXTON_LIBRARY']!,
    );
    try {
      final seen = <List<Map<String, dynamic>>>[];
      final first = Completer<void>();
      final second = Completer<void>();
      final sub = local.watch('Entry').listen((rows) {
        seen.add(rows);
        if (seen.length == 1) first.complete();
        if (seen.length == 2) second.complete();
      });
      await first.future;
      await local.direct({
        'model': 'Entry',
        'op': 'create',
        'identity': {'id': 'e'},
        'values': {'text': 'local'},
      });
      await second.future.timeout(const Duration(seconds: 1));
      expect((await local.read('Entry', {'id': 'e'}))?['text'], 'local');
      expect((seen.last.single)['text'], 'local');
      expect(await local.pendingTasks(), isEmpty);
      await sub.cancel();
    } finally {
      await local.close();
    }
  });
}

final class _TestWeak implements ActionWeakState {
  Object? value;
  _TestWeak(this.value);
  @override
  Object? get target => value;
}
