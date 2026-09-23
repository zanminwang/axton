import 'dart:convert';
import 'dart:io';
import 'package:axton/axton.dart';
import 'package:test/test.dart';

void main() {
  test(
    'prerequisite failure stays optimistic and explicit retry unlocks the push',
    () async {
      final dir = await Directory.systemTemp.createTemp(
        'axton-dart-prerequisite-',
      );
      final schema =
          jsonDecode(
                await File('../../fixtures/schemas/entry.json').readAsString(),
              )
              as Map<String, dynamic>;
      schema['prerequisites'] = [
        {
          'name': 'Upload',
          'fields': [
            {'name': 'key', 'type': 'String'},
          ],
        },
      ];
      schema['requirements'] = [
        {
          'model': 'Entry',
          'field': 'note',
          'name': 'Upload',
          'arguments': {'key': 'self'},
        },
      ];
      final client = await Client.open(
        path: '${dir.path}/db',
        schema: schema,
        libraryPath: Platform.environment['AXTON_LIBRARY']!,
      );
      try {
        await client.transaction(
          (tx) => tx.direct({
            'model': 'Entry',
            'op': 'create',
            'identity': {'id': 'e'},
            'values': {'text': 'A'},
          }),
        );
        await client.mutate({
          'name': 'Edit',
          'operations': [
            {
              'model': 'Entry',
              'op': 'update',
              'identity': {'id': 'e'},
              'values': {'note': 'asset'},
            },
          ],
        });
        var calls = 0;
        final handlers = <String, Future<void> Function(Map<String, dynamic>)>{
          'Upload': (args) async {
            expect(args['key'], anyOf('asset', 'second'));
            if (++calls == 1) throw StateError('offline');
          },
        };
        await client.runPrerequisites(handlers);
        expect((await client.read('Entry', {'id': 'e'}))?['note'], 'asset');
        expect(
          await client.freeze(),
          isNull,
          reason: 'a failed prerequisite blocks the push',
        );
        var task = (await client.pendingTasks()).single;
        expect(task['state'], 'failed');
        expect(task['name'], 'Upload');
        expect(task['error'], 'Bad state: offline');
        final status = await client.recordSyncState('Entry', {'id': 'e'});
        final prerequisite =
            (status['pending'] as List).first['prerequisites'].first;
        expect(prerequisite['error'], 'Bad state: offline');
        await client.setReadiness(task['key'] as String, 'pending');
        await client.runPrerequisites(handlers);
        expect(calls, 2);
        expect(await client.freeze(), isNotNull);
        expect(await client.pendingTasks(), isEmpty);
        await expectLater(
          client.runPrerequisites({}),
          completes,
          reason: 'nothing pending needs no handler',
        );
        // A task nobody handles fails with a reason instead of stopping the
        // run; once reset it is taken by a run that has the handler.
        await client.mutate({
          'name': 'Edit',
          'operations': [
            {
              'model': 'Entry',
              'op': 'update',
              'identity': {'id': 'e'},
              'values': {'note': 'second'},
            },
          ],
        });
        await client.runPrerequisites({});
        task = (await client.pendingTasks()).single;
        expect(task['state'], 'failed');
        expect(task['error'], 'missing prerequisite handler');
        await client.setReadiness(task['key'] as String, 'pending');
        await client.runPrerequisites(handlers);
        expect(calls, 3);
        expect(await client.pendingTasks(), isEmpty);
      } finally {
        await client.close();
        await dir.delete(recursive: true);
      }
    },
  );
}
