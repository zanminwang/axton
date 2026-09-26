import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:axton/axton.dart';
import 'package:axton/src/bridge.dart';
import 'package:test/test.dart';

/// A fresh temporary file with the Entry schema, opened through the real
/// native runtime as a [Client] or a bare [Bridge].
class Fixture {
  Fixture(this.dir, this.schema);
  final Directory dir;
  final Map<String, dynamic> schema;
  String get path => '${dir.path}/db';
  static String get library => Platform.environment['AXTON_LIBRARY']!;

  static Future<Fixture> create(String prefix) async {
    final dir = await Directory.systemTemp.createTemp(prefix);
    final schema =
        jsonDecode(
              await File('../../fixtures/schemas/entry.json').readAsString(),
            )
            as Map<String, dynamic>;
    return Fixture(dir, schema);
  }

  Future<Client> client() =>
      Client.open(path: path, schema: schema, libraryPath: library);
  Future<Bridge> bridge() =>
      Bridge.open(path: path, schema: schema, libraryPath: library);
  Future<void> dispose() => dir.delete(recursive: true);
}

Map<String, dynamic> create(String text) => {
  'model': 'Entry',
  'op': 'create',
  'identity': {'id': 'e'},
  'values': {'text': text},
};

Map<String, dynamic> update(String text) => {
  'model': 'Entry',
  'op': 'update',
  'identity': {'id': 'e'},
  'values': {'text': text},
};

Future<String?> text(Client client) async =>
    (await client.read('Entry', {'id': 'e'}))?['text'] as String?;

/// Whether [future] settles within [wait]: `'settled'` or `'pending'`.
Future<String> settles(Future<Object?> future, [int wait = 50]) => future
    .then<String>((_) => 'settled', onError: (Object _) => 'settled')
    .timeout(Duration(milliseconds: wait), onTimeout: () => 'pending');

void main() {
  late Fixture fixture;
  setUp(() async => fixture = await Fixture.create('axton-bridge-'));
  tearDown(() => fixture.dispose());

  test('overlapping tasks return to the waiter that submitted them', () async {
    final client = await fixture.client();
    try {
      await client.transaction((tx) => tx.direct(create('hello')));
      final gate = Completer<void>();
      final entered = Completer<void>();
      final order = <String>[];
      final transaction = client.transaction((tx) async {
        await tx.direct(update('inside'));
        expect((await tx.read('Entry', {'id': 'e'}))!['text'], 'inside');
        entered.complete();
        await gate.future;
        await tx.direct(update('committed'));
      });
      unawaited(transaction.then((_) => order.add('transaction')));
      await entered.future;
      final read = client.read('Entry', {'id': 'e'});
      final state = client.syncState();
      unawaited(read.then((_) => order.add('read')));
      unawaited(state.then((_) => order.add('syncState')));
      expect(await settles(read), 'pending');
      expect(await settles(state), 'pending');
      gate.complete();
      await transaction;
      expect((await read)!['text'], 'committed');
      expect((await state)['pending'], 0);
      expect(order, ['transaction', 'read', 'syncState']);
      expect(await text(client), 'committed');
    } finally {
      await client.close();
    }
  });

  test('a task after close fails client_closed; close is idempotent', () async {
    final bridge = await fixture.bridge();
    expect(bridge.opened['clientId'], isA<String>());
    await bridge.close();
    await expectLater(
      bridge.task({'kind': 'status'}),
      throwsA(
        isA<StateError>().having((e) => e.message, 'message', 'client_closed'),
      ),
    );
    await bridge.close();
    expect(Bridge.attached, isNot(contains(bridge.runtimeId)));
  });

  test(
    'a throwing callback rethrows the same object and rolls back; a caught failed command fails at commit',
    () async {
      final client = await fixture.client();
      try {
        await client.transaction((tx) => tx.direct(create('hello')));
        final thrown = _Thrown();
        Object? caught;
        try {
          await client.transaction((tx) async {
            await tx.direct(update('rolled back'));
            throw thrown;
          });
        } catch (error) {
          caught = error;
        }
        expect(identical(caught, thrown), isTrue);
        expect(await text(client), 'hello');

        Object? engine;
        Object? failed;
        try {
          await client.transaction((tx) async {
            try {
              await tx.direct(create('duplicate'));
            } catch (error) {
              engine = error;
            }
            await tx.direct(update('after failure'));
          });
        } catch (error) {
          failed = error;
        }
        expect(engine, isA<StateError>());
        expect(
          failed,
          isA<StateError>().having(
            (e) => e.message,
            'message',
            (engine! as StateError).message,
          ),
        );
        expect(await text(client), 'hello');
      } finally {
        await client.close();
      }
    },
  );

  test('nested savepoints carry the scope Rust issued for them', () async {
    final client = await fixture.client();
    try {
      await client.transaction((tx) async {
        await tx.direct(create('top'));
        await tx.savepoint(() async {
          await tx.direct(update('outer'));
          try {
            await tx.savepoint(() async {
              await tx.direct(update('inner'));
              throw StateError('inner');
            });
          } on StateError catch (_) {}
          expect((await tx.read('Entry', {'id': 'e'}))!['text'], 'outer');
          await tx.savepoint(() => tx.direct(update('released')));
        });
        expect((await tx.read('Entry', {'id': 'e'}))!['text'], 'released');
      });
      expect(await text(client), 'released');
    } finally {
      await client.close();
    }
  });

  test('a failed open throws and leaves nothing attached', () async {
    final before = Bridge.attached.toSet();
    await expectLater(
      Bridge.open(
        path: '${fixture.dir.path}/missing/dir/db',
        schema: fixture.schema,
        libraryPath: Fixture.library,
      ),
      throwsStateError,
    );
    expect(Bridge.attached.toSet(), before);
    final client = await fixture.client();
    try {
      await client.transaction((tx) => tx.direct(create('after')));
      expect(await text(client), 'after');
    } finally {
      await client.close();
    }
    expect(Bridge.attached.toSet(), before);
  });

  test('close during an open callback fails its later commands', () async {
    final client = await fixture.client();
    final gate = Completer<void>();
    final entered = Completer<void>();
    final later = Completer<Object>();
    final transaction = client.transaction((tx) async {
      await tx.direct(create('never'));
      entered.complete();
      await gate.future;
      try {
        await tx.read('Entry', {'id': 'e'});
        later.complete('succeeded');
      } catch (error) {
        later.complete(error);
      }
    });
    await entered.future;
    final closing = client.close();
    await expectLater(
      transaction,
      throwsA(
        isA<StateError>().having((e) => e.message, 'message', 'client_closed'),
      ),
    );
    gate.complete();
    expect(
      await later.future,
      isA<StateError>().having(
        (e) => e.message,
        'message',
        anyOf('transaction_closed', 'client_closed'),
      ),
    );
    await closing;
    final reopened = await fixture.client();
    try {
      expect(await text(reopened), isNull, reason: 'the open unit rolled back');
    } finally {
      await reopened.close();
    }
  });

  test('the bridge envelopes match the shared fixtures', () async {
    final fixtures =
        jsonDecode(
              await File('../../fixtures/bridge/envelopes.json').readAsString(),
            )
            as Map<String, dynamic>;
    final inputs = (fixtures['inputs'] as List).cast<Map<String, dynamic>>();
    final events = (fixtures['events'] as List).cast<Map<String, dynamic>>();
    for (final entry in [...inputs, ...events]) {
      expect(entry['type'], isA<String>(), reason: '$entry');
    }
    // Every input the bridge builds has the spelling Rust decodes.
    final built = [
      Bridge.taskEnvelope('42', {
        'kind': 'read',
        'key': {
          'model': 'Todo',
          'identity': {'id': 't'},
        },
      }),
      Bridge.taskEnvelope('44', {'kind': 'transaction'}),
      Bridge.transactionCommandEnvelope('43', 'tx7', 'sp1', {
        'kind': 'direct',
        'operation': {},
      }),
      Bridge.transactionCommandEnvelope('45', 'tx7', null, {
        'kind': 'savepoint',
      }),
      Bridge.callbackResultEnvelope('5', 'tx7', ok: true),
      Bridge.callbackResultEnvelope('5', 'tx7', ok: false, error: 'boom'),
      Bridge.effectResultEnvelope('101', ok: true, value: {'event': 'opened'}),
      Bridge.effectResultEnvelope(
        '101',
        ok: true,
        value: {'event': 'message', 'body': '{}'},
      ),
      Bridge.effectResultEnvelope(
        '101',
        ok: true,
        value: {'event': 'overflow'},
      ),
      Bridge.effectResultEnvelope('101', ok: true, value: {'event': 'closed'}),
      Bridge.effectResultEnvelope(
        '102',
        ok: false,
        error: 'pull failed',
        status: 401,
      ),
      Bridge.effectResultEnvelope('103', ok: false, error: 'offline'),
      Bridge.effectResultEnvelope(
        '104',
        ok: true,
        value: {'status': 200, 'body': '{}'},
      ),
      Bridge.closeEnvelope,
    ];
    expect(jsonDecode(jsonEncode(built)), inputs);
    // Every event carries what the dispatcher switches on.
    final seen = <String>{};
    for (final event in events) {
      final type = event['type'] as String;
      seen.add(type);
      switch (type) {
        case 'taskCompleted':
          expect(event['requestId'], isA<String>());
          expect(event['ok'], isA<bool>());
          expect(event.containsKey('value'), isTrue);
          if (event['ok'] == false) expect(event['error'], isA<String>());
          // A failure's machine-readable reason, when it has one.
          if (event.containsKey('details')) {
            expect((event['details'] as Map)['code'], isA<String>());
          }
        case 'effect':
          expect(event['effectId'], isA<String>());
          final operation = event['operation'] as Map<String, dynamic>;
          expect(operation['kind'], isA<String>());
          if (operation['kind'] == 'callback') {
            expect(operation['transactionId'], isA<String>());
            expect(operation['requestId'], isA<String>());
          }
        case 'cancelEffect':
          expect(event['effectId'], isA<String>());
        case 'callCompleted':
          expect(event['callId'], isA<String>());
          expect(event.containsKey('outcome'), isTrue);
        case 'observerChanged':
          expect(event['observerId'], isA<String>());
          expect(event.containsKey('snapshot'), isTrue);
        case 'report':
          expect((event['diagnostic'] as Map)['kind'], isA<String>());
        case 'changed':
          expect((event['tables'] as List).cast<String>(), isNotEmpty);
        case 'runtimeClosed':
          break;
        default:
          fail('unknown event type $type');
      }
    }
    expect(seen, {
      'taskCompleted',
      'effect',
      'cancelEffect',
      'callCompleted',
      'observerChanged',
      'report',
      'changed',
      'runtimeClosed',
    });
  });

  test(
    'a throwing changed listener cannot stop the completion in its batch',
    () async {
      final bridge = await fixture.bridge();
      try {
        final reported = <Object>[];
        final done = Completer<Object?>();
        runZonedGuarded(() {
          bridge.changed.listen((_) => throw StateError('listener'));
          bridge
              .task({'kind': 'direct', 'operation': create('hello')})
              .then(done.complete, onError: done.completeError);
        }, (error, _) => reported.add(error));
        await done.future.timeout(const Duration(seconds: 5));
        expect(reported, [
          isA<StateError>().having((e) => e.message, 'message', 'listener'),
        ]);
        final row = await bridge.task({
          'kind': 'read',
          'key': {
            'model': 'Entry',
            'identity': {'id': 'e'},
          },
        });
        expect((row as Map)['text'], 'hello');
      } finally {
        await bridge.close();
      }
    },
  );

  test(
    'an observer claimed at its task completion hears the snapshot of the same batch',
    () async {
      final bridge = await fixture.bridge();
      try {
        final heard = <Map<String, dynamic>>[];
        String? claimed;
        final value = await bridge.task(
          {'kind': 'scopeSubscribe', 'scope': 'book'},
          onValue: (value) {
            claimed = (value as Map)['observerId'] as String;
            expect(heard, isEmpty, reason: 'claimed before its first snapshot');
            bridge.listen(claimed!, heard.add);
          },
        );
        expect((value as Map)['observerId'], claimed);
        expect(heard, hasLength(1), reason: 'the first snapshot was not lost');
        expect(heard.single['kind'], 'subscription');
        expect((heard.single['status'] as Map)['connection'], 'offline');
        // The runtime's close ends the observer with its terminal snapshot.
        await bridge.close();
        expect(heard, hasLength(2));
        expect(heard.last['closed'], isTrue);
      } finally {
        await bridge.close();
      }
    },
  );

  test('a failure with a code carries it beside the message', () async {
    final bridge = await fixture.bridge();
    try {
      await expectLater(
        bridge.task({
          'kind': 'scopeBootstrap',
          'scope': 'book',
          'subscriptionId': 99,
        }),
        throwsA(
          isA<TaskFailure>()
              .having(
                (e) => e.message,
                'message',
                startsWith('subscription.closed'),
              )
              .having((e) => e.details, 'details', {
                'code': 'subscription.closed',
              }),
        ),
      );
      await expectLater(
        bridge.task({'kind': 'nope'}),
        throwsA(
          isA<StateError>().having(
            (e) => e is TaskFailure ? e.details : null,
            'details',
            isNull,
          ),
        ),
      );
    } finally {
      await bridge.close();
    }
  });

  test('a malformed envelope is reported and completes nothing', () async {
    final bridge = await fixture.bridge();
    try {
      final report = bridge.reports.first;
      bridge.submitRaw({'type': 'nope'});
      final diagnostic = await report.timeout(const Duration(seconds: 5));
      expect(diagnostic['kind'], 'protocol');
      expect(await bridge.task({'kind': 'status'}), isA<Map>());
    } finally {
      await bridge.close();
    }
  });
}

class _Thrown {}
