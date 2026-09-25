import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:axton/axton.dart';

Future<void> main(List<String> args) async {
  final schema =
      jsonDecode(await File('../../fixtures/schemas/entry.json').readAsString())
          as Map<String, dynamic>;
  final client = await Client.open(
    path: '${args[1]}/dart-live.sqlite',
    schema: schema,
    libraryPath: Platform.environment['AXTON_LIBRARY']!,
  );
  final writer = await Client.open(
    path: '${args[1]}/dart-writer.sqlite',
    schema: schema,
    libraryPath: Platform.environment['AXTON_LIBRARY']!,
  );
  final errors = <Object>[];
  Future<void> wait(Future<bool> Function() predicate) async {
    final end = DateTime.now().add(const Duration(seconds: 10));
    while (DateTime.now().isBefore(end)) {
      if (await predicate()) return;
      await Future<void>.delayed(const Duration(milliseconds: 5));
    }
    throw StateError('Dart live wait timed out: $errors');
  }

  Map<String, dynamic> edit(String id, String text) => {
    'name': 'Edit',
    'operations': [
      {
        'model': 'Entry',
        'op': 'update',
        'identity': {'id': id},
        'values': {'text': text},
      },
    ],
  };
  final live = SyncServer(url: args[0], token: () => 'demo-user');
  Future<void> initialized(Subscription subscription) => wait(
    () async =>
        subscription.status.initialization == SubscriptionInitialization.ready,
  );
  try {
    final connection = await client.connect(live, onError: errors.add);
    final subscription = await client.subscribe('book:demo');
    final writerConnection = await writer.connect(
      SyncServer(url: args[0], token: () => 'demo-user'),
      onError: errors.add,
    );
    final writerSubscription = await writer.subscribe('book:demo');
    // Each subscription starts at the head its own handshake acknowledged
    // (#150), so neither client holds what was published before it. READY tells
    // the harness to publish those records again; #151 owns loading a whole
    // Scope explicitly.
    await initialized(subscription);
    await initialized(writerSubscription);
    if ((await client.query('Entry')).isNotEmpty)
      throw StateError('a new subscription loaded records published before it');
    stdout.writeln('READY');
    await stdout.flush();
    await wait(() async => (await client.query('Entry')).length >= 56);
    await wait(() async => (await writer.query('Entry')).length >= 56);
    var seen = false;
    final watch = client.watch('Entry').listen((rows) {
      if (rows.any((r) => r['text'] == 'Dart second')) seen = true;
    });
    await client.mutate(edit('entry-1', ' Dart first '));
    await client.mutate(edit('entry-1', ' Dart second '));
    await wait(() async => (await client.syncState())['pending'] == 0 && seen);
    await connection.pause();
    await client.mutate(edit('entry-1', ' Dart offline '));
    await writer.mutate(edit('paged-54', 'Dart missed remote'));
    await wait(() async => (await writer.syncState())['pending'] == 0);
    await connection.resume();
    await wait(
      () async =>
          (await client.syncState())['pending'] == 0 &&
          (await client.read('Entry', {'id': 'paged-54'}))?['text'] ==
              'Dart missed remote',
    );
    if ((await client.read('Entry', {'id': 'entry-1'}))?['text'] !=
        'Dart offline')
      throw StateError('offline edit missing');
    // Unsubscribing removes only the registration: the records, their stamps and
    // the pending queue stay (guarantee D6). Registering again is a new
    // subscription identity, which starts over at the next acknowledged head and
    // reloads nothing.
    await client.unsubscribe('book:demo');
    if ((await client.query('Entry')).length < 56)
      throw StateError('unsubscribing removed local records');
    final recreated = await client.subscribe('book:demo');
    await initialized(recreated);
    if ((await client.query('Entry')).length < 56)
      throw StateError('a recreated subscription lost local records');
    await watch.cancel();
    await connection.close();
    await writerConnection.close();
    if (errors.isNotEmpty) throw StateError('$errors');
    print(
      'Dart live catch-up, watches, dependent push, reconnect and subscriptions: passed',
    );
  } finally {
    await client.close();
    await writer.close();
  }
}
