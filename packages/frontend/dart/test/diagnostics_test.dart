import 'dart:convert';
import 'dart:io';
import 'package:axton/axton.dart';
import 'package:axton/src/bindings/bridge.dart';
import 'package:test/test.dart';
import 'fake_carrier.dart';

class Sink implements AxtonDiagnostics {
  final void Function(AxtonTraceRecord) callback;
  Sink(this.callback);
  @override
  void record(AxtonTraceRecord record) => callback(record);
}

Map<String, dynamic> trace() => {
  'type': 'trace',
  'dropped': 2,
  'records': [
    {
      'schemaVersion': 1,
      'kind': 'span',
      'name': 'store.open',
      'source': 'rust',
      'clockDomain': 'native:test',
      'spanId': '1',
      'startUs': 10,
      'durationUs': 20,
      'attributes': {'rows': 3},
    },
  ],
};

void main() {
  test(
    'diagnostics is opt-in and native records preserve source timing and loss',
    () async {
      final carrier = FakeCarrier(), records = <AxtonTraceRecord>[];
      final bridge = await Bridge.open(
        path: 'fake',
        schema: {},
        stream: 'test',
        carrier: carrier,
        diagnostics: Sink(records.add),
      );
      expect(carrier.openedRequest['diagnostics'], true);
      carrier.publish([trace()]);
      await Future<void>.delayed(Duration.zero);
      expect(records.first.startUs, 10);
      expect(records.first.durationUs, 20);
      expect(records.last.kind, 'loss');
      expect(records.last.attributes['records'], 2);
      expect(
        () => records.first.attributes['rows'] = 4,
        throwsUnsupportedError,
      );
      await bridge.close();
      final off = FakeCarrier();
      final plain = await Bridge.open(
        path: 'fake',
        schema: {},
        stream: 'test',
        carrier: off,
      );
      expect(off.openedRequest.containsKey('diagnostics'), false);
      await plain.close();
    },
  );
  test(
    'a throwing receiver cannot swallow a following task completion',
    () async {
      final carrier = FakeCarrier(
        (request) => request['type'] == 'close'
            ? null
            : [trace(), completed(request['requestId'] as String, 42)],
      );
      final bridge = await Bridge.open(
        path: 'fake',
        schema: {},
        stream: 'test',
        carrier: carrier,
        diagnostics: Sink((_) {
          throw StateError('sink fault');
        }),
      );
      expect(await bridge.task({'kind': 'probe'}), 42);
      await bridge.close();
    },
  );
  test(
    'real native tracing leaves local commits and clients independent',
    () async {
      final dir = await Directory.systemTemp.createTemp('axton-diagnostics-');
      final schema =
          jsonDecode(
                await File(
                  '../../../fixtures/schemas/entry.json',
                ).readAsString(),
              )
              as Map<String, dynamic>;
      final records = <AxtonTraceRecord>[];
      Client? a, b;
      try {
        final applicationData =
            Platform.environment['AXTON_TEST_APPLICATION_DATA'];
        if (applicationData != null)
          Client.configureApplicationData(
            applicationData,
            libraryPath: Platform.environment['AXTON_LIBRARY'],
          );
        a = await Client.open(
          path: '${dir.path}/a',
          schema: schema,
          stream: 'User:viewer',
          libraryPath: Platform.environment['AXTON_LIBRARY'],
          diagnostics: Sink(records.add),
        );
        b = await Client.open(
          path: '${dir.path}/b',
          schema: schema,
          stream: 'User:viewer',
          libraryPath: Platform.environment['AXTON_LIBRARY'],
          diagnostics: Sink((_) {
            throw StateError('sink fault');
          }),
        );
        final operation = {
          'model': 'Entry',
          'op': 'create',
          'identity': {'id': 'e'},
          'values': {'text': 'kept', 'note': null},
        };
        await a.transaction((tx) => tx.direct(operation));
        await b.transaction((tx) => tx.direct(operation));
        expect((await a.read('Entry', {'id': 'e'}))?['text'], 'kept');
        expect((await b.read('Entry', {'id': 'e'}))?['text'], 'kept');
        expect(records.where((r) => r.name == 'store.open'), hasLength(1));
        expect(records.any((r) => r.name == 'sqlite.commit'), true);
        expect(
          records
              .where((r) => r.source == 'rust')
              .map((r) => r.clockDomain)
              .toSet(),
          hasLength(1),
        );
      } finally {
        await b?.close();
        await a?.close();
        await dir.delete(recursive: true);
      }
    },
  );
}
