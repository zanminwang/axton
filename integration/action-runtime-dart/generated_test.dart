import 'dart:convert';
import 'dart:io';

import 'package:test/test.dart';
import 'package:axton/axton.dart' as sdk;
import 'generated.dart';
import 'model_only/generated.dart' as model_only;
import 'model_free/generated.dart' as model_free;

void main() {
  late Directory directory;
  late GeneratedClient client;
  setUp(() async {
    directory = await Directory.systemTemp.createTemp(
      'axton-generated-action-',
    );
    client = await GeneratedClient.open(
      path: '${directory.path}/state.sqlite',
      libraryPath: Platform.environment['AXTON_DART_LIBRARY']!,
    );
  });
  tearDown(() async {
    await client.close();
    await directory.delete(recursive: true);
  });

  test(
    'standalone and transaction models write locally and notify watch',
    () async {
      final date = DateTime.utc(2026, 9, 23);
      final watched = client.models.note.watch().firstWhere(
        (rows) => rows.isNotEmpty,
      );
      await client.models.note.create(
        Note(id: 'n', at: date, mood: Mood.calm, label: null),
      );
      expect(
        (await watched.timeout(const Duration(seconds: 2))).single.at,
        date,
      );
      expect(
        (await client.models.note.get(const NoteIdentity(id: 'n')))?.mood,
        Mood.calm,
      );
      await client.models.note.update(
        const NoteIdentity(id: 'n'),
        const NotePatch(label: Present('updated')),
      );
      await client.transaction((tx) async {
        await tx.models.note.update(
          const NoteIdentity(id: 'n'),
          NotePatch(at: Present(date.add(const Duration(days: 1)))),
        );
        expect(
          (await tx.models.note.get(const NoteIdentity(id: 'n')))?.label,
          'updated',
        );
      });
      final record = (await client.models.note.query()).single;
      expect(record.at, date.add(const Duration(days: 1)));
      expect(record.label, 'updated');
      expect((await client.syncState())['pending'], 0);
      await client.models.note.delete(const NoteIdentity(id: 'n'));
      expect(await client.models.note.get(const NoteIdentity(id: 'n')), isNull);
    },
  );

  test(
    'generated durable pump and direct action decode shared SDK outcomes',
    () async {
      final at = DateTime.utc(2026, 9, 23, 12);
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      var pumps = 0;
      var directs = 0;
      final served = server.listen((request) async {
        final body = jsonDecode(await utf8.decoder.bind(request).join()) as Map;
        final result = {
          'result': at.toIso8601String(),
          'moods': ['calm', 'loud'],
          'maybe': null,
        };
        if (request.uri.path == '/sync/mutations') {
          pumps++;
          final mutation = (body['mutations'] as List).single as Map;
          request.response.write(
            jsonEncode({
              'clientId': body['clientId'],
              'batchSequence': body['batchSequence'],
              'rejections': [],
              'records': [],
              'completions': [
                {
                  'callId': mutation['callId'],
                  'outcome': {
                    'status': 'succeeded',
                    'result': mutation['name'] == 'Ping' ? null : result,
                  },
                },
              ],
            }),
          );
        } else if (request.uri.path == '/sync/actions') {
          directs++;
          final call = body['call'] as Map;
          if (call['name'] == 'Echo') {
            expect((call['args'] as Map)['at'], at.toIso8601String());
            expect((call['args'] as Map)['moods'], ['calm']);
          }
          request.response.write(
            jsonEncode({
              'completion': {
                'callId': call['callId'],
                'outcome': {
                  'status': 'succeeded',
                  'result': call['name'] == 'Ping' ? null : result,
                },
              },
              'records': [],
            }),
          );
        } else {
          request.response.statusCode = 404;
        }
        await request.response.close();
      });
      final connection = await client.connect(
        SyncServer(
          url: 'http://127.0.0.1:${server.port}',
          token: () => 'alice',
        ),
        directTimeout: const Duration(seconds: 2),
      );
      try {
        final ActionCall<EchoOutput> call = await client.actions.echo(
          at: at,
          moods: [Mood.calm],
          maybe: null,
        );
        final sdk.ActionCall<EchoOutput> sdkCall = call;
        expect(sdkCall.status, sdk.ActionStatus.pending);
        final ActionOutcome<EchoOutput> outcome = await call.wait().timeout(
          const Duration(seconds: 3),
        );
        final sdk.ActionOutcome<EchoOutput> sdkOutcome = outcome;
        expect(sdkOutcome, isA<sdk.ActionSuccess<EchoOutput>>());
        expect(outcome, isA<ActionSuccess<EchoOutput>>());
        expect((outcome as ActionSuccess<EchoOutput>).result.result, at);
        expect(outcome.result.moods, [Mood.calm, Mood.loud]);
        final ActionCall<void> pingCall = await client.actions.ping();
        final sdk.ActionCall<void> sdkPingCall = pingCall;
        final ActionOutcome<void> pingOutcome = await sdkPingCall
            .wait()
            .timeout(const Duration(seconds: 3));
        expect(pingOutcome, isA<ActionSuccess<void>>());
        expect(pingCall.status, ActionStatus.succeeded);
        final direct = await client.actions.call.echo(
          at: at,
          moods: [Mood.calm],
          maybe: null,
        );
        expect(direct.result, at);
        expect(direct.maybe, isNull);
        await client.actions.call.ping();
        expect(pumps, 2);
        expect(directs, 2);
        expect((await client.syncState())['pending'], 0);
      } finally {
        await connection.close();
        await served.cancel();
        await server.close(force: true);
      }
    },
  );

  test(
    'model-only and model-free generated clients execute through the SDK',
    () async {
      final only = await model_only.GeneratedClient.open(
        path: '${directory.path}/only.sqlite',
        libraryPath: Platform.environment['AXTON_DART_LIBRARY']!,
      );
      try {
        await only.models.item.create(
          const model_only.Item(id: 'i', label: 'Local'),
        );
        expect(
          (await only.models.item.get(
            const model_only.ItemIdentity(id: 'i'),
          ))?.label,
          'Local',
        );
        expect((await only.syncState())['pending'], 0);
      } finally {
        await only.close();
      }
      final free = await model_free.GeneratedClient.open(
        path: '${directory.path}/free.sqlite',
        libraryPath: Platform.environment['AXTON_DART_LIBRARY']!,
      );
      try {
        final model_free.ActionCall<void> call = await free.actions.ping();
        expect(call.status, model_free.ActionStatus.pending);
        expect((await free.syncState())['pending'], 1);
        await free.close();
        expect(await call.wait(), isA<model_free.ActionFailure<void>>());
      } finally {
        await free.close();
      }
    },
  );

  test('generated open forwards the direct timeout', () async {
    await expectLater(
      GeneratedClient.open(
        path: '${directory.path}/invalid-timeout.sqlite',
        libraryPath: Platform.environment['AXTON_DART_LIBRARY']!,
        server: SyncServer(url: 'http://127.0.0.1:1', token: () => 'alice'),
        directTimeout: Duration.zero,
      ),
      throwsArgumentError,
    );
  });

  test(
    'model operand and omitted update fields use generated wire codecs',
    () async {
      final at = DateTime.utc(2026, 9, 23);
      await client.actions.touch(
        note: Note(id: 'n', at: at, mood: Mood.loud, label: null),
        changed: const TouchChangedUpdate(id: 'n'),
      );
      await client.actions.touch(
        note: Note(id: 'other', at: at, mood: Mood.calm, label: null),
      );
      final frozen = jsonDecode((await client.client.freeze())!) as Map;
      final mutations = frozen['mutations'] as List;
      final args = (mutations.first as Map)['args'] as Map;
      final omitted = (mutations.last as Map)['args'] as Map;
      expect((args['note'] as Map)['at'], at.toIso8601String());
      expect((args['note'] as Map)['mood'], 'loud');
      expect(args['changed'], {'id': 'n'});
      expect(omitted['changed'], isNull);
    },
  );
}
