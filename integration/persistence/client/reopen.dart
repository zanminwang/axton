import 'dart:convert';
import 'dart:io';

import 'generated.dart';

void check(bool value, String message) {
  if (!value) throw StateError(message);
}

Future<void> main(List<String> args) async {
  final connection = StoreConnection(
    url: 'http://127.0.0.1:1',
    token: () => 'offline',
  );
  final library = Platform.environment['AXTON_DART_LIBRARY']!;
  for (var run = 0; run < 2; run++) {
    try {
      final unexpected = await GeneratedClient.open(
        path: args[0],
        stream: 'User:viewer',
        connection: connection,
        libraryPath: library,
      );
      await unexpected.close();
      throw StateError('legacy adoption unexpectedly succeeded');
    } catch (error) {
      check(
        error.toString().contains('unsupported Store format'),
        'legacy protocol refusal: $error',
      );
    }
  }
  String? first;
  for (var run = 0; run < 2; run++) {
    final c = await GeneratedClient.open(
      path: '${args[0]}.bound',
      stream: 'User:viewer',
      connection: connection,
      libraryPath: library,
    );
    await c.client.connection?.pause();
    try {
      if (run == 0) {
        await c.mutations.add(
          const AddInput(
            todo: TodoCreate(
              id: 'live',
              title: 'persisted',
              channel: 'opaque Channel',
            ),
          ),
        );
        await c.transaction(
          (tx) => tx.mutations.retitle(
            const RetitleInput(
              todo: RetitleTodoUpdate(
                id: 'live',
                title: Present('local edited'),
              ),
            ),
          ),
        );
        await c.mutations.edit(
          const EditInput(
            todo: EditTodoUpdate(
              id: 'live',
              channel: Present('pending Channel'),
            ),
          ),
        );
      }
      final row = await c.models.todo.get(const TodoIdentity(id: 'live'));
      check(
        row?.title == 'local edited' && row?.channel == 'pending Channel',
        'local and pending data',
      );
      // The two seed Mutations (Add, Retitle) and Edit.
      check((await c.syncState())['pending'] == 3, 'durable pending');
      final saved = await c.client.readSql(
        'SELECT q.*, d.descriptor AS retained_descriptor, o.* FROM axton_mutation_queue q JOIN axton_descriptor d ON d.context=q.descriptor JOIN axton_mutation_queue_operation o ON o.mutation_id=q.id ORDER BY q.id,o.step',
      );
      check(saved.length == 3, 'three saved intents');
      final bytes = jsonEncode(saved);
      if (run == 0) {
        first = bytes;
      } else {
        check(bytes == first, 'exact queued intent after reopen');
      }
    } finally {
      await c.close();
    }
  }
  print(
    'generated Dart: legacy adoption refused twice; bound local Mutations and exact queued intent survive reopen',
  );
}
