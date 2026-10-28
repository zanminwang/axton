import 'dart:async';
import 'dart:convert';
import 'dart:io';
import '../fixture/generated/generated.dart';
import 'package:axton/axton.dart' show Client;
import 'clock.dart';
export 'clock.dart' show sampleClock;

const textBytes = 512;
final fixtureText = 'x' * textBytes;
const fixtureStream = 'Benchmark:entries';
String identity(String prefix, int i) =>
    '$prefix-${i.toString().padLeft(8, '0')}';
void check(bool condition, String message) {
  if (!condition) throw StateError(message);
}

class Recorder implements AxtonDiagnostics {
  final Stopwatch clock;
  final String client;
  final records = <Map<String, Object?>>[];
  final clocks = <Map<String, Object?>>[];
  int dropped = 0;
  bool active = true;
  Recorder(this.clock, this.client);
  @override
  void record(AxtonTraceRecord record) {
    if (record.kind == 'clock') {
      clocks.add({...record.toJson(), 'client': client});
      return;
    }
    if (!active) return;
    if (records.length < 200000) {
      records.add({
        ...record.toJson(),
        'client': client,
        'receivedAtUs': clock.elapsedMicroseconds,
        'receivedAtReferenceUs': sampleClock.reference.elapsedMicroseconds,
      });
    } else {
      dropped++;
    }
    if (record.kind == 'loss')
      dropped += record.attributes['records'] as int? ?? 1;
  }
}

void verifyRows(List<Entry> rows, int seeds, int writes) {
  check(
    rows.length == seeds + writes,
    'expected ${seeds + writes} rows, got ${rows.length}',
  );
  final expected = {
    for (var i = 0; i < seeds; i++) identity('seed', i),
    for (var i = 0; i < writes; i++) identity('live', i),
  };
  for (final row in rows) {
    check(
      expected.remove(row.id),
      'unexpected or duplicate identity ${row.id}',
    );
    check(row.text == fixtureText, 'content mismatch ${row.id}');
  }
  check(expected.isEmpty, 'missing identities');
}

/// Emit progress before completion so timeouts retain partial timings/traces.
void progress(String phase, Map<String, Object?> data) =>
    stdout.writeln(jsonEncode({'type': 'progress', 'phase': phase, ...data}));

Future<void> awaitStart() async {
  progress('ready', {});
  final command = await sampleClock.command();
  check(command == 'go', 'runner start barrier');
}

Future<void> runSample(
  List<String> args,
  Future<Map<String, Object?>> Function(
    Map<String, dynamic>,
    Directory,
    Stopwatch,
    List<Recorder>,
  )
  body,
) async {
  final options = jsonDecode(args.single) as Map<String, dynamic>;
  final directory = Directory(options['directory'] as String);
  final recorders = <Recorder>[];
  final clock = Stopwatch();
  sampleClock = SampleClock();
  Timer? keepAlive;
  try {
    await sampleClock.synchronizeRunner('start');
    sampleClock.prepareNative();
    sampleClock.synchronizeDart('start');
    if (options['diagnostics'] == true) sampleClock.synchronizeNative('start');
    if (options['scenario'] == 'initial-sync')
      sampleClock.startMeasurement(clock);
    Client.configureApplicationData(
      '${directory.path}/application',
      libraryPath: Platform.environment['AXTON_DART_LIBRARY'],
    );
    options['applicationConfiguredUs'] = clock.elapsedMicroseconds;
    // Native pending Calls alone do not keep a headless Dart isolate alive.
    keepAlive = Timer.periodic(const Duration(seconds: 1), (_) {});
    final result = await body(options, directory, clock, recorders);
    sampleClock.synchronizeDart('end');
    if (options['diagnostics'] == true) sampleClock.synchronizeNative('end');
    await sampleClock.synchronizeRunner('end');
    check(recorders.every((r) => r.dropped == 0), 'incomplete diagnostics');
    stdout.writeln(
      jsonEncode({
        'type': 'result',
        'status': 'passed',
        ...result,
        'clockCorrelation': sampleClock.toJson(),
        'clockRecords': recorders.expand((r) => r.clocks).toList(),
        'diagnostics': recorders.expand((r) => r.records).toList(),
        'dropped': recorders.fold(0, (n, r) => n + r.dropped),
      }),
    );
  } catch (error, stack) {
    stdout.writeln(
      jsonEncode({
        'type': 'result',
        'status': 'failed',
        'error': '$error',
        'stack': '$stack',
        'clockCorrelation': sampleClock.toJson(),
        'clockRecords': recorders.expand((r) => r.clocks).toList(),
        'diagnostics': recorders.expand((r) => r.records).toList(),
      }),
    );
    exitCode = 1;
  } finally {
    keepAlive?.cancel();
    await sampleClock.input.cancel();
  }
}
