import 'dart:async';
import 'dart:io';
import '../fixture/generated/generated.dart';
import '../support/recorder.dart';

Future<void> main(List<String> args) => runSample(args, (
  options,
  dir,
  clock,
  recorders,
) async {
  final count = options['records'] as int;
  final recorder = Recorder(clock, 'A');
  recorders.add(recorder);
  final visible = Completer<void>();
  final timings = <String, int>{};
  StreamSubscription<List<Entry>>? watch;
  GeneratedClient? client;
  List<Entry> last = [];
  try {
    clock.start();
    timings['initializationStartUs'] = 0;
    timings['applicationConfiguredUs'] =
        options['applicationConfiguredUs'] as int;
    timings['storeOpenStartUs'] = clock.elapsedMicroseconds;
    client = await GeneratedClient.open(
      path: '${dir.path}/a.sqlite',
      stream: fixtureStream,
      libraryPath: Platform.environment['AXTON_DART_LIBRARY'],
      diagnostics: options['diagnostics'] == true ? recorder : null,
    );
    timings['storeOpenUs'] = clock.elapsedMicroseconds;
    watch = client.models.entry.watch().listen(
      (rows) {
        final at = clock.elapsedMicroseconds;
        last = rows;
        // The initial empty snapshot is deliberately excluded.
        if (!timings.containsKey('firstVisibleUs') &&
            rows.any((row) => row.id.startsWith('seed-'))) {
          timings['firstVisibleUs'] = at;
          progress('first.visible', {'atUs': at});
        }
        if (rows.length == count && !visible.isCompleted) {
          timings['allVisibleUs'] = at;
          progress('all.visible', {'atUs': at});
          visible.complete();
        }
      },
      onError: (Object error, StackTrace stack) {
        if (!visible.isCompleted) visible.completeError(error, stack);
      },
    );
    timings['connectStartUs'] = clock.elapsedMicroseconds;
    await client.connect(
      StoreConnection(url: options['url'] as String, token: () => 'benchmark'),
      onError: (error) => progress('connection.error', {'error': '$error'}),
    );
    timings['connectReturnedUs'] = clock.elapsedMicroseconds;
    timings['bootstrapRequestedUs'] = clock.elapsedMicroseconds;
    await client.bootstrap();
    timings['bootstrapCompleteUs'] = clock.elapsedMicroseconds;
    progress('bootstrap.complete', {'timings': timings});
    await visible.future;
    recorder.active = false;
    verifyRows(last, count, 0);
    verifyRows(await client.models.entry.query(), count, 0);
    final sync = await client.syncState();
    check(sync['pending'] == 0, 'initial sync has pending work');
    return {
      'timings': timings,
      'verifiedRows': count,
      'pending': sync['pending'],
    };
  } finally {
    await watch?.cancel();
    await client?.close();
  }
});
