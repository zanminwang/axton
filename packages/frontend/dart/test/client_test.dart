import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:axton/axton.dart';
import 'package:test/test.dart';
import 'store_fixture.dart';
import 'fake_transport.dart';

/// The named `Edit` of Entry `e`, the seed route for a text change.
Future<Call<void>> edit(SubmitMutationPort port, String text) =>
    port.submitMutation<void>('Edit', 1, {
      'entry': {'id': 'e', 'text': text},
    }, (_) {});
Map<String, dynamic> input(String id) => {
  'entry': {'id': id, 'text': 'published', 'note': null},
};
Matcher stateError(String message) =>
    throwsA(isA<StateError>().having((e) => e.message, 'message', message));
Matcher callError(String code) =>
    throwsA(isA<CallError>().having((e) => e.code, 'code', code));

void main() {
  late Directory dir;
  late Client client;
  late Map<String, dynamic> schema;
  Future<Client> open([String suffix = 'db']) => Client.open(
    path: '${dir.path}/$suffix',
    schema: schema,
    stream: 'User:viewer',
    connection: offlineStoreConnection(),
    libraryPath: Platform.environment['AXTON_LIBRARY']!,
  );
  Future<String?> text([String id = 'e']) async =>
      (await client.read('Entry', {'id': id}))?['text'] as String?;
  Future<Call<void>> publish(Transaction tx, String id) =>
      tx.submitMutation<void>('Publish', 1, input(id), (_) {});
  setUp(() async {
    dir = await Directory.systemTemp.createTemp('axton-dart-client-');
    schema =
        jsonDecode(
              await File('../../../fixtures/schemas/entry.json').readAsString(),
            )
            as Map<String, dynamic>;
    declareEntryEdit(schema);
    final entry = (schema['models'] as List).single;
    (schema['actions'] as List).addAll(<Map<String, dynamic>>[
      {
        'kind': 'mutation',
        'name': 'Publish',
        'version': 1,
        'input': {
          'models': [entry],
          'enums': [],
        },
        'inputs': [
          {
            'kind': 'model',
            'name': 'entry',
            'model': 'Entry',
            'operation': 'create',
            'cardinality': 'single',
          },
        ],
        'outputs': [],
        'requirements': [],
        'prerequisites': [],
      },
      {
        'kind': 'mutation',
        'name': 'Ping',
        'version': 1,
        'inputs': [],
        'outputs': [],
      },
    ]);
    client = await open();
    // A delivered row: the base no pending count below includes.
    await deliverRecords(client, [
      {
        'model': 'Entry',
        'identity': {'id': 'e'},
        'state': {'text': 'hello', 'note': null},
      },
    ]);
  });
  tearDown(() async {
    await client.close();
    await dir.delete(recursive: true);
  });

  test(
    'committed pending Call wait refuses own transaction and preserves terminal wait',
    () async {
      final call = await client.submitMutation<void>('Ping', 1, {}, (_) {});
      final outside = call.wait();
      await client.transaction((_) async {
        await expectLater(
          call.wait().timeout(const Duration(milliseconds: 200)),
          stateError('transaction_active'),
        );
        expect(call.status, CallStatus.pending);
      });
      await client.resetStore(discardPending: true);
      expect(((await outside) as CallFailure<void>).error.code, 'abandoned');
      await client.transaction((_) async {
        expect(
          ((await call.wait()) as CallFailure<void>).error.code,
          'abandoned',
        );
      });
    },
  );
  test(
    'local transaction reads its own writes and throw rolls them back',
    () async {
      await expectLater(
        client.transaction((tx) async {
          await edit(tx, 'bad');
          expect((await tx.read('Entry', {'id': 'e'}))!['text'], 'bad');
          throw StateError('rollback');
        }),
        stateError('rollback'),
      );
      expect(await text(), 'hello');
      expect((await client.syncState())['pending'], 0);
    },
  );
  test(
    'nested client transaction is refused without running its body',
    () async {
      var ran = false;
      await client.transaction((tx) async {
        await expectLater(
          client.transaction((_) async {
            ran = true;
          }),
          stateError('transaction_active'),
        );
        expect((await tx.read('Entry', {'id': 'e'}))!['text'], 'hello');
      });
      expect(ran, isFalse);
    },
  );
  test(
    'captured outer client commands reject promptly inside its callback',
    () async {
      await client.transaction((tx) async {
        await expectLater(
          client
              .read('Entry', {'id': 'e'})
              .timeout(const Duration(milliseconds: 200)),
          stateError('transaction_active'),
        );
        await expectLater(
          client.submitMutation<void>('Ping', 1, {}, (_) {}),
          stateError('transaction_active'),
        );
        await edit(tx, 'inside');
      });
      expect(await text(), 'inside');
    },
  );
  test(
    'savepoint rolls back its writes while prior and later writes commit',
    () async {
      await client.transaction((tx) async {
        await edit(tx, 'before');
        await expectLater(
          tx.savepoint(() async {
            await edit(tx, 'discard');
            throw StateError('undo');
          }),
          stateError('undo'),
        );
        expect((await tx.read('Entry', {'id': 'e'}))!['text'], 'before');
        await edit(tx, 'after');
      });
      expect(await text(), 'after');
    },
  );
  test('escaped transaction cannot change committed state', () async {
    late Transaction escaped;
    await client.transaction((tx) async {
      escaped = tx;
    });
    await expectLater(edit(escaped, 'escaped'), throwsStateError);
    expect(await text(), 'hello');
  });
  test('unawaited native write poisons the entire transaction', () async {
    await expectLater(
      client.transaction((tx) async {
        unawaited(edit(tx, 'unawaited').then((_) {}, onError: (Object _) {}));
      }),
      throwsStateError,
    );
    expect(await text(), 'hello');
  });
  test(
    'several named calls commit together and stay independently durable',
    () async {
      final calls = await client.transaction(
        (tx) async => [await publish(tx, 'p'), await publish(tx, 'q')],
      );
      expect(calls.map((c) => c.status), everyElement(CallStatus.pending));
      expect((await client.syncState())['pending'], 2);
      await client.close();
      client = await open();
      expect((await client.syncState())['pending'], 2);
      expect(await text('p'), 'published');
      expect(await text('q'), 'published');
    },
  );
  test(
    'Call leaked from rolled-back transaction fails without committing optimism',
    () async {
      late Call<void> call;
      await expectLater(
        client.transaction((tx) async {
          call = await publish(tx, 'p');
          throw StateError('undo');
        }),
        stateError('undo'),
      );
      await expectLater(call.wait(), callError('transaction_rolled_back'));
      expect(await text('p'), isNull);
      expect((await client.syncState())['pending'], 0);
    },
  );
  test(
    'a Mutation built from a read commits with its transaction and a rollback discards the Call',
    () async {
      Future<Call<void>> copy(Transaction tx) async {
        final row = await tx.read('Entry', {'id': 'e'});
        final call = await tx.submitMutation<void>('Publish', 1, {
          'entry': {'id': 'copy', 'text': row!['text'], 'note': null},
        }, (_) {});
        expect((await tx.read('Entry', {'id': 'copy'}))!['text'], 'hello');
        return call;
      }

      late Call<void> rolledBack;
      await expectLater(
        client.transaction((tx) async {
          rolledBack = await copy(tx);
          throw StateError('undo');
        }),
        stateError('undo'),
      );
      await expectLater(
        rolledBack.wait(),
        callError('transaction_rolled_back'),
      );
      expect(await text('copy'), isNull);
      expect((await client.syncState())['pending'], 0);
      final committed = await client.transaction(copy);
      expect(committed.status, CallStatus.pending);
      expect(await text('copy'), 'hello');
      expect((await client.syncState())['pending'], 1);
    },
  );
  test('savepoint ends only Calls owned by its scope', () async {
    late Call<void> discarded;
    final kept = await client.transaction((tx) async {
      final kept = await publish(tx, 'p');
      await expectLater(
        tx.savepoint(() async {
          discarded = await publish(tx, 'q');
          throw StateError('undo');
        }),
        stateError('undo'),
      );
      await expectLater(discarded.wait(), callError('transaction_rolled_back'));
      await expectLater(kept.wait(), callError('transaction_uncommitted'));
      return kept;
    });
    expect(kept.status, CallStatus.pending);
    expect((await client.syncState())['pending'], 1);
    expect(await text('p'), 'published');
    expect(await text('q'), isNull);
  });
  for (final savepoint in [false, true]) {
    test(
      'foreign captured ${savepoint ? 'empty savepoint' : 'write'} refuses before body/admission and poisons its owner',
      () async {
        final other = await open('other');
        var ran = false;
        try {
          await expectLater(
            client.transaction((a) async {
              await other.transaction((b) async {
                if (savepoint) {
                  await expectLater(
                    a.savepoint(() async {
                      ran = true;
                    }),
                    stateError('foreign transaction scope'),
                  );
                } else {
                  await expectLater(
                    publish(a, 'foreign'),
                    stateError('foreign transaction scope'),
                  );
                }
              });
            }),
            stateError('foreign transaction scope'),
          );
          expect(ran, isFalse);
          expect(await text('foreign'), isNull);
          expect((await client.syncState())['pending'], 0);
        } finally {
          await other.close();
        }
      },
    );
  }
  test('independent other-file ordinary reads remain available', () async {
    final other = await open('other');
    try {
      await client.transaction((tx) async {
        expect(await other.read('Entry', {'id': 'e'}), isNull);
        await edit(tx, 'inside');
      });
      expect(await text(), 'inside');
    } finally {
      await other.close();
    }
  });
  test(
    'offline schema evolution retains named queue and local view under a new materialization',
    () async {
      await client.submitMutation<void>('Edit', 1, {
        'entry': {'id': 'e', 'text': 'pending'},
      }, (_) {});
      await client.close();
      schema = jsonDecode(jsonEncode(schema)) as Map<String, dynamic>;
      final entry = (schema['models'] as List).single as Map<String, dynamic>;
      final oldEntry = jsonDecode(jsonEncode(entry)) as Map<String, dynamic>;
      entry['version'] = 2;
      (entry['fields'] as List).add({
        'name': 'extra',
        'nullable': true,
        'type': {'kind': 'scalar', 'name': 'string'},
      });
      schema['resultModels'] = [
        {...oldEntry, 'version': 1, 'enums': []},
        {...entry, 'version': 2, 'enums': []},
      ];
      (schema['models'] as List).add({
        'name': 'NewModel',
        'version': 1,
        'bootstrap': true,
        'identity': ['id'],
        'fields': [
          {
            'name': 'id',
            'nullable': false,
            'type': {'kind': 'scalar', 'name': 'string'},
          },
        ],
      });
      client = await open();
      expect((await client.syncState())['pending'], 1);
      expect(await text(), 'pending');
      expect((await client.read('Entry', {'id': 'e'}))!['extra'], isNull);
    },
  );
  test(
    'one physical owner, ordinary close and offline reopen retain local state',
    () async {
      await expectLater(open(), throwsA(anything));
      await edit(client, 'retained');
      await client.close();
      await client.close();
      client = await open();
      expect(await text(), 'retained');
    },
  );
  test(
    'watch publishes initial and distinct committed local views and closes',
    () async {
      final rows = <List<Map<String, dynamic>>>[];
      final done = Completer<void>();
      final sub = client.watch('Entry').listen(rows.add, onDone: done.complete);
      await Future<void>.delayed(Duration.zero);
      await edit(client, 'changed');
      await edit(client, 'changed');
      await Future<void>.delayed(Duration.zero);
      expect(rows.map((r) => r.single['text']), ['hello', 'changed']);
      await client.close();
      await done.future;
      await sub.cancel();
    },
  );
  test(
    'filtered watch ignores unrelated rows and failed watch reports an error',
    () async {
      final rows = <List<Map<String, dynamic>>>[];
      final sub = client.watch('Entry', where: {'id': 'e'}).listen(rows.add);
      await Future<void>.delayed(Duration.zero);
      await client.submitMutation<void>('Publish', 1, {
        'entry': {'id': 'other', 'text': 'irrelevant', 'note': null},
      }, (_) {});
      await edit(client, 'filtered');
      await Future<void>.delayed(Duration.zero);
      expect(rows.map((r) => r.single['text']), ['hello', 'filtered']);
      await sub.cancel();
      await expectLater(client.watch('Missing').first, throwsA(anything));
    },
  );
}
