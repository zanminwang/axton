import 'package:axton/axton.dart';

/// Explicit disconnected transport fixture. Offline open needs no connection.
StoreConnection offlineStoreConnection() => StoreConnection(
  url: 'http://127.0.0.1:1',
  token: () => 'offline',
  onError: (_) {},
);

/// Declare the named update used by generic-runtime tests; its slot carries
/// only Entry identity and explicitly mutable fields, like compiler output.
void declareEntryEdit(Map<String, dynamic> schema) {
  final entry = (schema['models'] as List).cast<Map>().firstWhere(
    (m) => m['name'] == 'Entry',
  );
  schema['actions'] = [
    {
      'kind': 'mutation',
      'name': 'Edit',
      'version': 1,
      'input': {
        'enums': [],
        'models': [entry],
      },
      'inputs': [
        {
          'name': 'entry',
          'kind': 'model',
          'model': 'Entry',
          'operation': 'update',
          'cardinality': 'single',
          'allowedPatchFields': ['text', 'note'],
        },
      ],
      'outputs': [],
      'outputEnums': [],
      'prerequisites': schema['prerequisites'] ?? [],
      'requirements': schema['requirements'] ?? [],
      'sequence': null,
    },
  ];
}

/// Declare the test Mutations `Create<Model>` and `Update<Model>` for each
/// [models] entry of [schema] (all of them when null), so tests seed and
/// change rows with Mutations. Each takes one `row` slot; an update may patch
/// every non-identity field. Prerequisites and requirements are copied from
/// the schema, like [declareEntryEdit].
void declareRowMutations(Map<String, dynamic> schema, [List<String>? models]) {
  final actions = (schema['actions'] ??= <Object>[]) as List;
  for (final model in (schema['models'] as List).cast<Map>()) {
    if (models != null && !models.contains(model['name'])) continue;
    final identity = (model['identity'] as List).cast<String>();
    for (final operation in ['create', 'update']) {
      actions.add({
        'kind': 'mutation',
        'name':
            '${operation == 'create' ? 'Create' : 'Update'}${model['name']}',
        'version': 1,
        'input': {
          'enums': schema['enums'] ?? [],
          'models': [model],
        },
        'inputs': [
          {
            'name': 'row',
            'kind': 'model',
            'model': model['name'],
            'operation': operation,
            'cardinality': 'single',
            if (operation == 'update')
              'allowedPatchFields': [
                for (final field in (model['fields'] as List).cast<Map>())
                  if (!identity.contains(field['name'])) field['name'],
              ],
          },
        ],
        'outputs': [],
        'outputEnums': [],
        'prerequisites': schema['prerequisites'] ?? [],
        'requirements': schema['requirements'] ?? [],
        'sequence': null,
      });
    }
  }
}

/// Create one [model] row offline through `Create<Model>` (see
/// [declareRowMutations]); it stays a pending Mutation.
Future<Call<void>> createRow(
  SubmitMutationPort port,
  String model,
  Map<String, dynamic> row,
) => port.submitMutation<void>('Create$model', 1, {'row': row}, (_) {});

/// Patch one [model] row offline through `Update<Model>`; [row] carries the
/// identity and the changed fields.
Future<Call<void>> updateRow(
  SubmitMutationPort port,
  String model,
  Map<String, dynamic> row,
) => port.submitMutation<void>('Update$model', 1, {'row': row}, (_) {});
