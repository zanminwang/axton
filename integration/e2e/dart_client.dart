import 'dart:convert';
import 'dart:io';
import 'package:axton/axton.dart';

Future<void> main(List<String> args) async {
  final schema =
      jsonDecode(await File('../../fixtures/schemas/entry.json').readAsString())
          as Map<String, dynamic>;
  final client = await Client.open(
    path: '${args[1]}/dart.sqlite',
    schema: schema,
    libraryPath: Platform.environment['AXTON_LIBRARY']!,
  );
  try {
    await client.subscribe('book:demo');
    final connection = await client.connect(
      SyncServer(url: args[0], token: () => 'demo-user'),
    );
    for (
      var i = 0;
      i < 500 && (await client.read('Entry', {'id': 'entry-1'})) == null;
      i++
    ) {
      await Future<void>.delayed(const Duration(milliseconds: 10));
    }
    final initial = await client.read('Entry', {'id': 'entry-1'});
    if (initial?['text'] != 'resumed')
      throw StateError('Dart initial pull mismatch: $initial');
    await client.mutate({
      'name': 'Edit',
      'operations': [
        {
          'model': 'Entry',
          'op': 'update',
          'identity': {'id': 'entry-1'},
          'values': {'text': '  from Dart  '},
        },
      ],
    });
    for (var i = 0; i < 200 && (await client.syncState())['pending'] != 0; i++) {
      await Future<void>.delayed(const Duration(milliseconds: 10));
    }
    await connection.close();
    final row = await client.read('Entry', {'id': 'entry-1'});
    final status = await client.syncState();
    if (row?['text'] != 'from Dart' || status['pending'] != 0)
      throw StateError('Dart settlement mismatch: $row $status');
    print('Dart -> Rust -> HTTP -> Rust -> Prisma -> SQLite: passed');
  } finally {
    await client.close();
  }
}
