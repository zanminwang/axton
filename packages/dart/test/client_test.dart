import 'dart:convert';
import 'dart:async';
import 'dart:io';
import 'package:axton/axton.dart';
import 'package:test/test.dart';

/// One client over a fresh temporary file with the Entry schema, plus a way to
/// reopen the same file. Every clause below starts from its own copy.
class Fixture {
  Fixture(this.dir, this.schema);
  final Directory dir;
  final Map<String, dynamic> schema;
  String get path => '${dir.path}/db';

  static Future<Fixture> create(String prefix) async {
    final dir = await Directory.systemTemp.createTemp(prefix);
    final schema =
        jsonDecode(
              await File('../../fixtures/schemas/entry.json').readAsString(),
            )
            as Map<String, dynamic>;
    return Fixture(dir, schema);
  }

  Future<Client> open() => Client.open(
    path: path,
    schema: schema,
    libraryPath: Platform.environment['AXTON_LIBRARY']!,
  );

  Future<void> dispose() => dir.delete(recursive: true);
}

Map<String, dynamic> update(String text) => {
  'model': 'Entry',
  'op': 'update',
  'identity': {'id': 'e'},
  'values': {'text': text},
};

/// Creates Entry `e` with text `hello` and reads it back inside the transaction.
Future<void> seed(Client client) => client.transaction((tx) async {
  await tx.direct({
    'model': 'Entry',
    'op': 'create',
    'identity': {'id': 'e'},
    'values': {'text': 'hello'},
  });
  expect((await tx.read('Entry', {'id': 'e'}))!['text'], 'hello');
});

Future<String?> text(Client client) async =>
    (await client.read('Entry', {'id': 'e'}))?['text'] as String?;

void main() {
  test('default native loader is reserved for iOS process symbols', () async {
    if (Platform.isIOS) return;
    await expectLater(
      Client.open(path: 'unused', schema: const {}),
      throwsA(
        isA<StateError>().having(
          (error) => error.message,
          'message',
          contains('libraryPath'),
        ),
      ),
    );
  });

  group('Dart callbacks through native Rust', () {
    late Fixture fixture;
    late Client client;
    setUp(() async {
      fixture = await Fixture.create('axton-dart-test-');
      client = await fixture.open();
      await seed(client);
    });
    tearDown(() async {
      await client.close();
      await fixture.dispose();
    });

    test('a transaction reads its own writes; a throw rolls it back', () async {
      await expectLater(
        client.transaction((tx) async {
          await tx.direct(update('bad'));
          expect((await tx.read('Entry', {'id': 'e'}))!['text'], 'bad');
          throw StateError('rollback');
        }),
        throwsStateError,
      );
      expect(await text(client), 'hello');
    });

    test(
      'raw transaction has no mutation method and captured client calls reject promptly',
      () async {
        final gate = Completer<void>();
        final entered = Completer<void>();
        final transaction = client.transaction((tx) async {
          expect(
            () => (tx as dynamic).mutate({'name': 'Edit'}),
            throwsNoSuchMethodError,
          );
          entered.complete();
          final outcome = await client
              .mutate({
                'name': 'Edit',
                'operations': [update('inside')],
              })
              .then(
                (_) => 'committed',
                onError: (Object error) => error.toString(),
              )
              .timeout(
                const Duration(milliseconds: 200),
                onTimeout: () => 'timeout',
              );
          expect(outcome, contains('transaction_active'));
          await gate.future;
        });
        try {
          await entered.future;
          final independent = client.mutate({
            'name': 'Edit',
            'operations': [update('outside')],
          });
          final state = await independent
              .then(
                (_) => 'committed',
                onError: (Object error) => error.toString(),
              )
              .timeout(
                const Duration(milliseconds: 50),
                onTimeout: () => 'queued',
              );
          expect(state, 'queued');
          gate.complete();
          await transaction;
          expect(await independent, 1);
          expect(await text(client), 'outside');
        } finally {
          if (!gate.isCompleted) gate.complete();
          await transaction;
        }
      },
    );

    test(
      'failed standalone enqueue leaves no queue entry or optimistic record',
      () async {
        await expectLater(
          client.mutate({
            'name': 'Broken',
            'operations': [
              {
                'model': 'Entry',
                'op': 'create',
                'identity': {'id': 'failed'},
                'values': {'text': 'optimistic'},
              },
              {
                'model': 'Missing',
                'op': 'create',
                'identity': {'id': 'missing'},
                'values': {'text': 'invalid'},
              },
            ],
          }),
          throwsStateError,
        );
        expect((await client.syncState())['pending'], 0);
        expect(await client.read('Entry', {'id': 'failed'}), isNull);
        expect(
          await client.mutate({
            'name': 'Edit',
            'operations': [update('after')],
          }),
          1,
        );
      },
    );

    test('an unawaited native call fails the transaction', () async {
      await expectLater(
        client.transaction((tx) async {
          tx.direct(update('forgotten'));
        }),
        throwsStateError,
      );
      expect(await text(client), 'hello');
    });

    test(
      'a failed native call fails the transaction even when caught',
      () async {
        await expectLater(
          client.transaction((tx) async {
            try {
              await tx.direct({
                'model': 'Entry',
                'op': 'create',
                'identity': {'id': 'e'},
                'values': {'text': 'duplicate'},
              });
            } catch (_) {}
          }),
          throwsStateError,
        );
        expect(await text(client), 'hello');
      },
    );

    test('a savepoint confines its rollback to its own scope', () async {
      await client.transaction((tx) async {
        try {
          await tx.savepoint(() async {
            await tx.direct(update('savepoint'));
            throw StateError('rollback');
          });
        } catch (_) {}
        expect((await tx.read('Entry', {'id': 'e'}))!['text'], 'hello');
        await tx.direct(update('after savepoint'));
      });
      expect(await text(client), 'after savepoint');
    });

    test(
      'a child savepoint finishing after its parent fails the transaction',
      () async {
        await expectLater(
          client.transaction((tx) async {
            final gate = Completer<void>();
            Future<void>? child;
            try {
              await tx.savepoint(() async {
                await tx.direct(update('outer'));
                child = tx
                    .savepoint<void>(() async {
                      await gate.future;
                      throw StateError('late child');
                    })
                    .catchError((Object _) {});
              });
            } catch (_) {}
            gate.complete();
            await child;
          }),
          throwsStateError,
        );
        expect(await text(client), 'hello');
      },
    );

    test(
      'a frozen batch and committed state survive reopen; a closed handle refuses reads',
      () async {
        await client.mutate({
          'name': 'Edit',
          'operations': [update('offline')],
        });
        final frozen = await client.freeze();
        await client.close();
        client = await fixture.open();
        expect(await client.freeze(), frozen);
        expect(await text(client), 'offline');
        await client.close();
        await expectLater(client.read('Entry', {'id': 'e'}), throwsStateError);
      },
    );
  });

  test(
    'client close waits for connection setup and remains idempotent',
    () async {
      final fixture = await Fixture.create('axton-dart-close-');
      final client = await fixture.open();
      final errors = <Object>[];
      try {
        final starting = client.connect(
          SyncServer(url: 'http://127.0.0.1:1', token: () => 'secret'),
          onError: errors.add,
        );
        await Future.wait([starting, client.close()]);
        await Future<void>.delayed(Duration.zero);
        expect(errors, isEmpty);
        await client.close();
        await (await starting).close();
        await expectLater(
          client.connect(
            SyncServer(url: 'http://127.0.0.1:1', token: () => 'secret'),
          ),
          throwsStateError,
        );
      } finally {
        await client.close();
        await fixture.dispose();
      }
    },
  );

  test(
    'an incompatible schema keeps unsent work in the old file until rebuild is asked to leave it',
    () async {
      final fixture = await Fixture.create('axton-dart-rebuild-');
      final breaking =
          jsonDecode(jsonEncode(fixture.schema)) as Map<String, dynamic>;
      (breaking['models'][0]['fields'] as List).add({
        'name': 'due',
        'nullable': false,
        'type': {'kind': 'scalar', 'name': 'string'},
      });
      try {
        var client = await fixture.open();
        expect((await client.syncState())['schema']['rebuilt'], false);
        await seed(client);
        await client.mutate({
          'name': 'Edit',
          'operations': [update('offline')],
        });
        expect(await client.freeze(), isNotNull);
        await client.close();

        client = await Client.open(
          path: fixture.path,
          schema: breaking,
          libraryPath: Platform.environment['AXTON_LIBRARY']!,
        );
        var status = await client.syncState();
        expect(status['schema']['rebuilt'], false);
        expect(status['schema']['pending']['pending'], 1);
        expect(status['schema']['pending']['reason'], contains('due'));
        await expectLater(client.rebuild(), throwsStateError);
        final report = await client.rebuild(discardPending: true);
        expect(report['leftPending'], 1);
        expect(report['newFile'], endsWith('db.1'));
        status = await client.syncState();
        expect(status['schema']['rebuilt'], true);
        expect(status['schema']['pending'], isNull);
        expect(await text(client), isNull);
        expect(File(fixture.path).existsSync(), isTrue);
        await client.close();
      } finally {
        await fixture.dispose();
      }
    },
  );
}
