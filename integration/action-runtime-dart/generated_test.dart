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
      stream: 'User:viewer', connection: offline(),
      libraryPath: Platform.environment['AXTON_DART_LIBRARY']!,
    );
  });
  tearDown(() async {
    await client.close();
    await directory.delete(recursive: true);
  });

  test('typed input codecs retain millisecond dates, enums and nulls', () {
    final at = DateTime.utc(2026, 9, 23, 12);
    expect(EchoInput(at: at, moods: [Mood.calm, Mood.loud], maybe: null).toRecord(), {'at': '2026-09-23T12:00:00.000Z', 'moods': ['calm', 'loud'], 'maybe': null});
    expect(TouchInput(note: NoteCreate(id: 'n', at: at, mood: Mood.calm, label: null), changed: null).toRecord()['changed'], isNull);
  });
  test('a Mutation built from a read commits with its transaction and stays durable', () async {
    final at = DateTime.utc(2026, 9, 23);
    final call = await client.transaction((tx) async {
      expect(await tx.models.note.get(const NoteIdentity(id: 'n')), isNull);
      final call = await tx.mutations.touch(TouchInput(note: NoteCreate(id: 'n', at: at, mood: Mood.calm, label: null), changed: null));
      await expectLater(call.wait(), throwsA(isA<sdk.CallError>().having((e) => e.code, 'code', 'transaction_uncommitted')));
      return call;
    });
    expect(call.status, sdk.CallStatus.pending);
    expect((await client.models.note.get(const NoteIdentity(id: 'n')))!.at, at);
    await client.close();
    expect(((await call.wait()) as sdk.CallFailure).error.code, 'client.closed');
    client = await GeneratedClient.open(path: '${directory.path}/state.sqlite', stream: 'User:viewer', connection: offline(), libraryPath: Platform.environment['AXTON_DART_LIBRARY']!);
    expect((await client.syncState())['pending'], 1);
    expect((await client.models.note.get(const NoteIdentity(id: 'n')))!.at, at);
  });
  test('model-only facade reads without remote namespaces', () async {
    final only = await model_only.GeneratedClient.open(path: '${directory.path}/only', stream: 'User:viewer', connection: offline(), libraryPath: Platform.environment['AXTON_DART_LIBRARY']!);
    try {
      expect(await only.models.item.get(const model_only.ItemIdentity(id: 'i')), isNull);
      expect(() => (only as dynamic).mutations, throwsNoSuchMethodError);
      expect(() => (only as dynamic).queries, throwsNoSuchMethodError);
    } finally { await only.close(); }
  });
  test('model-free facade retains durable named scalar writes', () async {
    final free = await model_free.GeneratedClient.open(path: '${directory.path}/free', stream: 'User:viewer', connection: offline(), libraryPath: Platform.environment['AXTON_DART_LIBRARY']!);
    try {
      final call = await free.mutations.ping(const model_free.PingInput());
      expect(call.status, sdk.CallStatus.pending);
      expect((await free.syncState())['pending'], 1);
      expect(() => (free.models as dynamic).note, throwsNoSuchMethodError);
      expect(() => (free.queries as dynamic).enqueue, throwsNoSuchMethodError);
      await free.close();
      expect(((await call.wait()) as sdk.CallFailure).error.code, 'client.closed');
    } finally { await free.close(); }
  });
}
sdk.StoreConnection offline() => sdk.StoreConnection(url: 'http://127.0.0.1:1', token: () => 'offline', onError: (_) {});
