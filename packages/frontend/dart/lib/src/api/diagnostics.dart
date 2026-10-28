import 'dart:io' show pid;
import 'dart:math' show Random;

/// Optional performance instrumentation. Receivers must return promptly;
/// exceptions are ignored and cannot change a sync outcome.
abstract interface class AxtonDiagnostics {
  void record(AxtonTraceRecord record);
}

/// Schema 1 trace data. Stage names/attributes are experimental. Times are
/// microseconds within [clockDomain]. Cross-domain placement requires measured
/// clock correlation; a bridge's receipt timestamp is not the source timestamp.
class AxtonTraceRecord {
  final String kind;
  final String name;
  final String source;
  final String clockDomain;
  final int startUs;
  final int? durationUs;
  final String? spanId;
  final String? parentSpanId;
  final Map<String, Object?> attributes;
  const AxtonTraceRecord({
    required this.kind,
    required this.name,
    required this.source,
    required this.clockDomain,
    required this.startUs,
    this.durationUs,
    this.spanId,
    this.parentSpanId,
    this.attributes = const {},
  });
  factory AxtonTraceRecord.fromJson(Map<String, dynamic> value) {
    if (value['schemaVersion'] != 1)
      throw FormatException('Unsupported AXTON trace schema');
    return AxtonTraceRecord(
      kind: value['kind'] as String,
      name: value['name'] as String,
      source: value['source'] as String,
      clockDomain: value['clockDomain'] as String,
      startUs: value['startUs'] as int,
      durationUs: value['durationUs'] as int?,
      spanId: value['spanId'] as String?,
      parentSpanId: value['parentSpanId'] as String?,
      attributes: Map.unmodifiable(
        value['attributes'] as Map<String, dynamic>? ?? const {},
      ),
    );
  }
  Map<String, Object?> toJson() => {
    'schemaVersion': 1,
    'kind': kind,
    'name': name,
    'source': source,
    'clockDomain': clockDomain,
    'startUs': startUs,
    if (durationUs != null) 'durationUs': durationUs,
    if (spanId != null) 'spanId': spanId,
    if (parentSpanId != null) 'parentSpanId': parentSpanId,
    'attributes': attributes,
  };
}

/// SDK-internal recorder, instantiated only when diagnostics was supplied.
class TraceEmitter {
  final AxtonDiagnostics receiver;
  final String clockDomain;
  static final Stopwatch _clock = Stopwatch()..start();
  static final String isolateClockDomain =
      'dart-isolate:$pid:${Random.secure().nextInt(1 << 32)}:${Random.secure().nextInt(1 << 32)}';
  static int isolateClockUs() => _clock.elapsedMicroseconds;
  int _next = 0;
  TraceEmitter(this.receiver, String clockDomain)
    : clockDomain = '$isolateClockDomain:$clockDomain' {
    deliver(
      AxtonTraceRecord(
        kind: 'clock',
        name: 'clock.origin',
        source: 'dart',
        clockDomain: this.clockDomain,
        startUs: isolateClockUs(),
        attributes: {
          'referenceClockDomain': isolateClockDomain,
          'offsetLowerUs': 0,
          'offsetUpperUs': 0,
          'method': 'shared-isolate-Stopwatch-origin',
        },
      ),
    );
  }

  void deliver(AxtonTraceRecord record) {
    try {
      receiver.record(record);
    } catch (_) {
      /* Observations never decide outcomes. */
    }
  }

  void native(List<dynamic> records, int dropped) {
    for (final record in records) {
      deliver(AxtonTraceRecord.fromJson(record as Map<String, dynamic>));
    }
    if (dropped > 0)
      deliver(
        AxtonTraceRecord(
          kind: 'loss',
          name: 'diagnostics.dropped',
          source: 'dart',
          clockDomain: clockDomain,
          startUs: _clock.elapsedMicroseconds,
          attributes: {'records': dropped},
        ),
      );
  }

  TraceSpan start(
    String name, [
    Map<String, Object?> attributes = const {},
    TraceSpan? parent,
  ]) => TraceSpan._(
    this,
    name,
    '${++_next}',
    _clock.elapsedMicroseconds,
    attributes,
    parent?.id,
  );
}

class TraceSpan {
  final TraceEmitter emitter;
  final String name;
  final String id;
  final int startUs;
  final Map<String, Object?> attributes;
  final String? parentSpanId;
  bool _ended = false;
  TraceSpan._(
    this.emitter,
    this.name,
    this.id,
    this.startUs,
    this.attributes,
    this.parentSpanId,
  );
  void end([Map<String, Object?> result = const {}]) {
    if (_ended) return;
    _ended = true;
    emitter.deliver(
      AxtonTraceRecord(
        kind: 'span',
        name: name,
        source: 'dart',
        clockDomain: emitter.clockDomain,
        startUs: startUs,
        durationUs: TraceEmitter._clock.elapsedMicroseconds - startUs,
        spanId: id,
        parentSpanId: parentSpanId,
        attributes: Map.unmodifiable({...attributes, ...result}),
      ),
    );
  }
}
