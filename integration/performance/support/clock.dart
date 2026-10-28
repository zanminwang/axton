import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';
import 'package:axton/src/api/diagnostics.dart' show TraceEmitter;

typedef _NativeClock = Uint64 Function();
typedef _Clock = int Function();

/// The reference runs through setup and measurement. The scenario Stopwatch can
/// still start at zero after both live clients have initialized.
class SampleClock {
  final reference = Stopwatch()..start();
  final dartClockDomain = TraceEmitter.isolateClockDomain;
  final input = StreamIterator<String>(
    stdin.transform(utf8.decoder).transform(const LineSplitter()),
  );
  final records = <Map<String, Object?>>[];
  final nativeProbes = <String, List<Map<String, int>>>{};
  final dartProbes = <String, List<Map<String, int>>>{};
  final setupSpans = <Map<String, Object?>>[];
  late final referenceDomain = 'benchmark.reference:$pid';
  late final measurementDomain = 'benchmark.measurement:$pid';
  _Clock? _native;
  _Clock? _nativeId;
  bool _measurementStarted = false;

  Future<String> command() async {
    if (!await input.moveNext()) throw StateError('runner input closed');
    return input.current;
  }

  Future<void> synchronizeRunner(String phase) async {
    stdout.writeln(
      jsonEncode({
        'type': 'progress',
        'phase': 'clock.ready',
        'calibrationPhase': phase,
        'clockDomain': referenceDomain,
      }),
    );
    while (true) {
      final request = jsonDecode(await command()) as Map<String, dynamic>;
      final receive = reference.elapsedMicroseconds;
      if (request['type'] == 'clock.done') return;
      if (request['type'] != 'clock.probe')
        throw StateError('clock probe expected');
      final send = reference.elapsedMicroseconds;
      stdout.writeln(
        jsonEncode({
          'type': 'progress',
          'phase': 'clock.reply',
          'calibrationPhase': phase,
          'id': request['id'],
          'remoteReceiveUs': receive,
          'remoteSendUs': send,
        }),
      );
    }
  }

  void startMeasurement(Stopwatch clock) {
    if (_measurementStarted) return;
    final before = reference.elapsedMicroseconds;
    clock.start();
    final local = clock.elapsedMicroseconds;
    final after = reference.elapsedMicroseconds;
    _measurementStarted = true;
    records.add({
      'schemaVersion': 1,
      'kind': 'clock',
      'name': 'clock.origin',
      'source': 'benchmark',
      'clockDomain': measurementDomain,
      'startUs': local,
      'attributes': {
        'referenceClockDomain': referenceDomain,
        'offsetLowerUs': before - local - 1,
        'offsetUpperUs': after - local + 1,
        'method': 'shared-isolate-Stopwatch-bracket',
        'probes': [
          {'beforeUs': before, 'localUs': local, 'afterUs': after},
        ],
      },
    });
  }

  void synchronizeNative(String phase) {
    prepareNative();
    nativeProbes[phase] = List.generate(8, (_) {
      final before = reference.elapsedMicroseconds;
      final native = _native!();
      final after = reference.elapsedMicroseconds;
      return {
        'localSendUs': before,
        'remoteReceiveUs': native,
        'remoteSendUs': native,
        'localReceiveUs': after,
      };
    });
  }

  void synchronizeDart(String phase) {
    dartProbes[phase] = List.generate(8, (_) {
      final before = reference.elapsedMicroseconds;
      final remote = TraceEmitter.isolateClockUs();
      final after = reference.elapsedMicroseconds;
      return {
        'localSendUs': before,
        'remoteReceiveUs': remote,
        'remoteSendUs': remote,
        'localReceiveUs': after,
      };
    });
  }

  void prepareNative() {
    if (_native != null) return;
    final start = reference.elapsedMicroseconds;
    final library = DynamicLibrary.open(
      Platform.environment['AXTON_DART_LIBRARY']!,
    );
    _native = library.lookupFunction<_NativeClock, _Clock>(
      'axton_diagnostics_clock_us',
    );
    _nativeId = library.lookupFunction<_NativeClock, _Clock>(
      'axton_diagnostics_clock_id',
    );
    setupSpans.add({
      'kind': 'span',
      'name': 'Native library preparation (excluded setup)',
      'source': 'setup',
      'clockDomain': referenceDomain,
      'startUs': start,
      'durationUs': reference.elapsedMicroseconds - start,
      'lane': 'Excluded setup',
      'attributes': {'excludedFromMeasurement': true},
    });
  }

  Map<String, Object?> toJson() => {
    'referenceClockDomain': referenceDomain,
    'measurementClockDomain': measurementDomain,
    'nativeClockDomain': _nativeId == null
        ? null
        : 'native:$pid:${_nativeId!()}',
    'dartClockDomain': dartClockDomain,
    'clockRecords': records,
    'nativeProbes': nativeProbes,
    'dartProbes': dartProbes,
    'setupSpans': setupSpans,
  };
}

late SampleClock sampleClock;
