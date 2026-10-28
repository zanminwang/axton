import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:axton/axton.dart';
import 'package:axton/src/bindings/bridge.dart';
import 'package:axton/src/api/diagnostics.dart' show TraceEmitter;
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
    'SDK collectors share a monotonic clock and retain explicit span parents',
    () {
      final records = <AxtonTraceRecord>[];
      final a = TraceEmitter(Sink(records.add), 'a');
      final b = TraceEmitter(Sink(records.add), 'b');
      final anchors = records.where((r) => r.kind == 'clock').toList();
      expect(anchors, hasLength(2));
      expect(
        anchors[0].attributes['referenceClockDomain'],
        anchors[1].attributes['referenceClockDomain'],
      );
      expect(
        anchors.every(
          (r) =>
              r.attributes['offsetLowerUs'] == 0 &&
              r.attributes['offsetUpperUs'] == 0,
        ),
        true,
      );
      final parent = a.start('parent');
      final child = a.start('child', const {}, parent);
      child.end();
      parent.end();
      final other = b.start('other');
      other.end();
      final spans = records.where((r) => r.kind == 'span').toList();
      expect(spans[0].parentSpanId, spans[1].spanId);
      expect(spans[2].startUs, greaterThanOrEqualTo(spans[1].startUs));
      expect(spans[2].parentSpanId, isNull);
    },
  );
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
      final native = records.firstWhere((r) => r.name == 'store.open');
      expect(native.startUs, 10);
      expect(native.durationUs, 20);
      expect(
        records.singleWhere((r) => r.kind == 'loss').attributes['records'],
        2,
      );
      expect(() => native.attributes['rows'] = 4, throwsUnsupportedError);
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
      StreamSubscription<List<Map<String, dynamic>>>? watch;
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
        final first = Completer<void>(), changed = Completer<void>();
        watch = a.watch('Entry').listen((rows) {
          if (!first.isCompleted) first.complete();
          if (rows.any((r) => r['id'] == 'e') && !changed.isCompleted)
            changed.complete();
        });
        await first.future;
        await a.transaction((tx) => tx.direct(operation));
        await changed.future;
        for (final name in [
          'watch.read',
          'watch.convert',
          'watch.compare',
          'watch.snapshot',
          'bridge.nativeDrain',
          'bridge.decode',
        ]) {
          expect(
            records.any((r) => r.name == name && r.durationUs != null),
            true,
            reason: name,
          );
        }
        expect(records.any((r) => r.name == 'watch.invalidate'), true);
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
        await watch?.cancel();
        await b?.close();
        await a?.close();
        await dir.delete(recursive: true);
      }
    },
  );
}
