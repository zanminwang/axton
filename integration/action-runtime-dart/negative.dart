import 'generated.dart';

Future<void> invalid(
  GeneratedClient client,
  ActionCall<EchoOutput> call,
) async {
  await client.actions.echo(at: 'string', moods: [Mood.calm], maybe: null);
  await client.actions.echo(
    at: DateTime.utc(2026),
    moods: ['calm'],
    maybe: null,
  );
  await client.actions.touch(
    note: Note(id: 'n', at: DateTime.utc(2026), mood: Mood.calm, label: null),
    changed: TouchChangedUpdate(id: 'n', mood: Present(Mood.loud)),
  );
  await client.transaction((tx) async {
    tx.models.note.watch();
  });
  call.result;
}
