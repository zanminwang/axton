import 'dart:async';
import 'dart:io';
import 'package:axton/axton.dart' show AxtonReport;
import '../fixture/generated/generated.dart';
import '../support/recorder.dart';

Future<void> main(List<String> args) => runSample(args, (
  options,
  dir,
  clock,
  recorders,
) async {
  final seeds = options['records'] as int, writes = options['writes'] as int;
  final aRecorder = Recorder(clock, 'A'), bRecorder = Recorder(clock, 'B');
  recorders.addAll([aRecorder, bRecorder]);
  GeneratedClient? a, b;
  StreamSubscription<List<Entry>>? watch;
  final events = <String, Map<String, Object?>>{};
  final ready = Completer<void>(), complete = Completer<void>();
  List<Entry> last = [];
  final settlements = <Future<void>>[];
  final recordReports = <Map<String, Object?>>[];
  try {
    a = await GeneratedClient.open(
      path: '${dir.path}/a.sqlite',
      stream: fixtureStream,
      libraryPath: Platform.environment['AXTON_DART_LIBRARY'],
      diagnostics: options['diagnostics'] == true ? aRecorder : null,
    );
    b = await GeneratedClient.open(
      path: '${dir.path}/b.sqlite',
      stream: fixtureStream,
      libraryPath: Platform.environment['AXTON_DART_LIBRARY'],
      diagnostics: options['diagnostics'] == true ? bRecorder : null,
    );
    for (final client in [a, b]) {
      await client.connect(
        StoreConnection(
          url: options['url'] as String,
          token: () => 'benchmark',
        ),
        onError: (error) {
          if (error is AxtonReport) {
            final report = <String, Object?>{
              'client': identical(client, a) ? 'A' : 'B',
              'kind': error.kind,
              'model': error.model,
              'identity': error.identity,
              'ordinal': error.ordinal,
              'atUs': clock.elapsedMicroseconds,
              'duringMeasurement': clock.isRunning,
            };
            recordReports.add(report);
            progress('record.report', report);
          } else {
            progress('connection.error', {'error': '$error'});
          }
        },
      );
      await client.bootstrap();
      verifyRows(await client.models.entry.query(), seeds, 0);
    }
    watch = b.models.entry.watch().listen(
      (rows) {
        final at = clock.elapsedMicroseconds;
        last = rows;
        if (!clock.isRunning) {
          if (rows.length == seeds && !ready.isCompleted) ready.complete();
          return;
        }
        for (final row in rows) {
          final event = events[row.id];
          if (event != null) event.putIfAbsent('bVisibleUs', () => at);
        }
        if (rows.length == seeds + writes && !complete.isCompleted)
          complete.complete();
      },
      onError: (Object error, StackTrace stack) {
        if (!ready.isCompleted) ready.completeError(error, stack);
        if (!complete.isCompleted) complete.completeError(error, stack);
      },
    );
    await ready.future;
    // Both clients initialized and B's watch registered. Setup traces are
    // excluded from the live interval, as well as all fixture work.
    aRecorder.records.clear();
    bRecorder.records.clear();
    await awaitStart();
    clock.start();
    for (var i = 0; i < writes; i++) {
      final id = identity('live', i);
      final event = <String, Object?>{
        'id': id,
        'startUs': clock.elapsedMicroseconds,
      };
      events[id] = event;
      final call = await a.mutations.createEntry(
        CreateEntryInput(
          entry: EntryCreate(id: id, text: fixtureText),
        ),
      );
      event['localCommitUs'] = clock.elapsedMicroseconds;
      settlements.add(
        call.wait().then((outcome) {
          event['settlementUs'] = clock.elapsedMicroseconds;
          check(
            outcome is CallSuccess<CreateEntryOutput>,
            'expected accepted settlement $id',
          );
          if (outcome is CallSuccess<CreateEntryOutput>) {
            check(
              outcome.result.entry.id == id &&
                  outcome.result.entry.text == fixtureText,
              'settlement content $id',
            );
          }
          event['accepted'] = true;
        }),
      );
    }
    final localWritesCompleteUs = clock.elapsedMicroseconds;
    progress('local.complete', {'events': events.values.toList()});
    await Future.wait(settlements);
    final allSettledUs = clock.elapsedMicroseconds;
    await complete.future;
    aRecorder.active = false;
    bRecorder.active = false;
    final allVisibleUs = events.values
        .map((e) => e['bVisibleUs'] as int)
        .reduce((a, b) => a > b ? a : b);
    verifyRows(last, seeds, writes);
    verifyRows(await a.models.entry.query(), seeds, writes);
    verifyRows(await b.models.entry.query(), seeds, writes);
    check((await a.syncState())['pending'] == 0, 'A queue drained');
    check(
      events.values.every(
        (e) => e['accepted'] == true && e.containsKey('bVisibleUs'),
      ),
      'every write observed and settled',
    );
    return {
      'events': events.values.toList(),
      'recordReports': recordReports,
      'timings': {
        'localWritesCompleteUs': localWritesCompleteUs,
        'allSettledUs': allSettledUs,
        'allVisibleUs': allVisibleUs,
      },
      'verifiedRows': seeds + writes,
      'accepted': writes,
    };
  } finally {
    await watch?.cancel();
    await b?.close();
    await a?.close();
  }
});
