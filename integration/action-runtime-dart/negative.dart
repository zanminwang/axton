import 'generated.dart';

Future<void> invalid(
  GeneratedClient client,
  Call<EchoOutput> call,
) async {
  await client.mutations.echo(at: 'string', moods: [Mood.calm], maybe: null);
  await client.mutations.echo(
    at: DateTime.utc(2026),
    moods: ['calm'],
    maybe: null,
  );
  await client.mutations.touch(
    note: Note(id: 'n', at: DateTime.utc(2026), mood: Mood.calm, label: null),
    changed: TouchChangedUpdate(id: 'n', mood: Present(Mood.loud)),
  );
  await client.transaction((tx) async {
    tx.models.note.watch();
  });
  call.result;
  await client.actions.ping();
  final Call<NowOutput> direct = await client.queries.now(at: DateTime.utc(2026));
  await client.transaction((tx) async {
    tx.queries;
  });
  direct.hashCode;
}
