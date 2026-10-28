import 'dart:async';
import 'package:axton/axton.dart';
import 'package:axton/src/api/model_watch.dart';
import 'package:test/test.dart';
import 'fake_carrier.dart';

Map<String, dynamic> full(int version, List<Map<String, dynamic>> rows) => {
  'kind': 'watch',
  'protocol': 'splice-v1',
  'version': version,
  'rows': rows,
};
Map<String, dynamic> patch(int base, List<Map<String, dynamic>> splices) => {
  'kind': 'watch',
  'protocol': 'splice-v1',
  'baseVersion': base,
  'version': base + 1,
  'splices': splices,
};
Map<String, dynamic> splice(
  int index,
  int deleteCount, [
  List<Map<String, dynamic>> rows = const [],
]) => {'index': index, 'deleteCount': deleteCount, 'rows': rows};
Map<String, dynamic> changed(String id, Map<String, dynamic> snapshot) => {
  'type': 'observerChanged',
  'observerId': id,
  'snapshot': snapshot,
};
Future<void> turn() => Future<void>.delayed(Duration.zero);

Future<(Client, FakeCarrier)> fixture({
  Map<String, dynamic>? initial,
  bool delayed = false,
}) async {
  var issued = 0;
  final carrier = FakeCarrier((envelope) {
    final command = envelope['command'] as Map?;
    if (command == null) return null;
    final request = envelope['requestId'] as String;
    if (command['kind'] == 'watch' || command['kind'] == 'watchSql') {
      final id = '${++issued}';
      return delayed
          ? <Map<String, dynamic>>[]
          : [
              completed(request, {'observerId': id}),
              if (initial != null) changed(id, initial),
            ];
    }
    return [completed(request)];
  });
  final client = await Client.open(
    path: 'unused',
    schema: {},
    stream: 'User:u',
    carrier: carrier,
  );
  return (client, carrier);
}

void main() {
  test(
    'Model watch opts in and isolates nested rows across sequential splices',
    () async {
      final (client, carrier) = await fixture(
        initial: full(1, [
          {
            'id': 'a',
            'nested': {
              'items': [
                {'value': 'canonical'},
              ],
            },
          },
          {'id': 'b'},
          {'id': 'c'},
        ]),
      );
      addTearDown(client.close);
      final seen = <List<Map<String, dynamic>>>[];
      final errors = <Object>[];
      final watching = client
          .watch('Entry', where: {'active': true})
          .listen(seen.add, onError: errors.add);
      addTearDown(watching.cancel);
      await turn();
      expect(carrier.commands.first, {
        'kind': 'watch',
        'model': 'Entry',
        'spec': {
          'filter': {'active': true},
          'delivery': 'splice-v1',
        },
      });
      final first = seen.single;
      first.first['nested']['items'][0]['value'] = 'mutated';
      first.add({'id': 'extra'});
      carrier.publish([
        changed(
          '1',
          patch(1, [
            splice(1, 1, [
              {'id': 'd'},
            ]),
            splice(2, 0, [
              {'id': 'e'},
            ]),
            splice(3, 1),
          ]),
        ),
      ]);
      await turn();
      expect(seen.last, [
        {
          'id': 'a',
          'nested': {
            'items': [
              {'value': 'canonical'},
            ],
          },
        },
        {'id': 'd'},
        {'id': 'e'},
      ]);
      expect(first.map((row) => row['id']), ['a', 'b', 'c', 'extra']);
      expect(errors, isEmpty);
    },
  );
  test(
    'typed watch decodes only changed rows and copies retained lists',
    () async {
      final (client, carrier) = await fixture(
        initial: full(1, [
          {
            'id': 'a',
            'values': [
              [1, 2],
            ],
          },
          {'id': 'b', 'values': <Object>[]},
        ]),
      );
      addTearDown(client.close);
      var decoded = 0;
      final seen = <List<Map<String, dynamic>>>[], errors = <Object>[];
      final Stream<List<Map<String, dynamic>>> stream = client
          .watchDecoded<Map<String, dynamic>>(
            'Entry',
            decode: (Map<String, dynamic> row) {
              decoded++;
              return row;
            },
            copy: (Map<String, dynamic> row) => <String, dynamic>{
              'id': row['id'],
              'values': [
                for (final List list in row['values']) [...list],
              ],
            },
          );
      final watching = stream.listen(seen.add, onError: errors.add);
      addTearDown(watching.cancel);
      await turn();
      final first = seen.single;
      first.first['values'][0][0] = 99;
      carrier.publish([
        changed(
          '1',
          patch(1, [
            splice(1, 1, [
              {'id': 'c', 'values': <Object>[]},
            ]),
            splice(2, 0, [
              {'id': 'd', 'values': <Object>[]},
            ]),
          ]),
        ),
      ]);
      await turn();
      expect(decoded, 4);
      expect(seen.last.first['values'], [
        [1, 2],
      ]);
      expect(first.map((row) => row['id']), ['a', 'b']);
      expect(errors, isEmpty);
    },
  );

  test(
    'invalid patches request recovery once and resume from a newer full',
    () async {
      final (client, carrier) = await fixture(
        initial: full(1, [
          {'id': 'a'},
          {'id': 'b'},
        ]),
      );
      addTearDown(client.close);
      final seen = <List<Map<String, dynamic>>>[], errors = <Object>[];
      final watching = client
          .watch('Entry')
          .listen(seen.add, onError: errors.add);
      addTearDown(watching.cancel);
      await turn();
      expect((carrier.commands.first['spec'] as Map)['delivery'], 'splice-v1');
      carrier.publish([
        changed(
          '1',
          patch(1, [
            splice(0, 1, [
              {'id': 'partial'},
            ]),
            splice(8, 0, [
              {'id': 'invalid'},
            ]),
          ]),
        ),
      ]);
      await turn();
      expect(errors, hasLength(1));
      expect(seen, hasLength(1));
      expect(carrier.commands.where((c) => c['kind'] == 'watchSnapshot'), [
        {'kind': 'watchSnapshot', 'observerId': '1'},
      ]);
      carrier.publish([
        changed(
          '1',
          patch(1, [
            splice(0, 1, [
              {'id': 'ignored'},
            ]),
          ]),
        ),
      ]);
      await turn();
      expect(errors, hasLength(1));
      expect(seen, hasLength(1));
      carrier.publish([
        changed(
          '1',
          full(5, [
            {'id': 'a'},
            {'id': 'b'},
          ]),
        ),
        changed(
          '1',
          patch(5, [
            splice(1, 1, [
              {'id': 'recovered'},
            ]),
          ]),
        ),
      ]);
      await turn();
      expect(seen.map((rows) => rows.map((row) => row['id']).toList()), [
        ['a', 'b'],
        ['a', 'b'],
        ['a', 'recovered'],
      ]);
    },
  );

  for (final (name, message) in <(String, Map<String, dynamic>)>[
    ('patch before full', patch(1, [])),
    ('wrong base', patch(2, [])),
    ('version gap', {...patch(1, []), 'version': 3}),
    ('unsafe version', {...full(2, []), 'version': 9007199254740992}),
    ('nonpositive version', full(0, [])),
    ('fractional version', {...full(2, []), 'version': 1.5}),
    ('unknown protocol', {...full(2, []), 'protocol': 'future'}),
    ('negative index', patch(1, [splice(-1, 0)])),
    (
      'fractional index',
      patch(1, [
        {...splice(0, 0), 'index': .5},
      ]),
    ),
    ('negative delete count', patch(1, [splice(0, -1)])),
    ('excess delete count', patch(1, [splice(0, 2)])),
    (
      'invalid row',
      patch(1, [
        {
          ...splice(0, 0),
          'rows': [null],
        },
      ]),
    ),
    ('ambiguous message', {...full(2, []), 'splices': []}),
  ]) {
    test('Model watch recovers from $name', () async {
      final (client, carrier) = await fixture(
        initial: name == 'patch before full'
            ? null
            : full(1, [
                {'id': 'canonical'},
              ]),
      );
      addTearDown(client.close);
      final seen = <List<Map<String, dynamic>>>[], errors = <Object>[];
      final watching = client
          .watch('Entry')
          .listen(seen.add, onError: errors.add);
      addTearDown(watching.cancel);
      await turn();
      expect((carrier.commands.first['spec'] as Map)['delivery'], 'splice-v1');
      carrier.publish([changed('1', message)]);
      await turn();
      expect(errors, hasLength(1));
      expect(
        carrier.commands.where((c) => c['kind'] == 'watchSnapshot'),
        hasLength(1),
      );
      carrier.publish([
        changed(
          '1',
          full(3, [
            {'id': 'reset'},
          ]),
        ),
      ]);
      await turn();
      expect(seen.last, [
        {'id': 'reset'},
      ]);
    });
  }

  test('legacy snapshots and SQL keep full delivery', () async {
    final (client, carrier) = await fixture(
      initial: {
        'kind': 'watch',
        'rows': [
          {'id': 'a'},
        ],
      },
    );
    addTearDown(client.close);
    final seen = <List<Map<String, dynamic>>>[];
    final watching = client.watch('Entry').listen(seen.add);
    addTearDown(watching.cancel);
    await turn();
    carrier.publish([
      changed('1', {
        'kind': 'watch',
        'rows': [
          {'id': 'b'},
        ],
      }),
    ]);
    await turn();
    expect(seen.map((rows) => rows.single['id']), ['a', 'b']);
    final sql = <List<Map<String, dynamic>>>[];
    final statement = client.watchSql('SELECT id FROM Entry').listen(sql.add);
    addTearDown(statement.cancel);
    await turn();
    carrier.publish([
      changed('2', {
        'kind': 'watch',
        'rows': [
          {'id': 'c'},
        ],
      }),
    ]);
    await turn();
    expect(carrier.commands.where((c) => c['kind'] == 'watchSql').single, {
      'kind': 'watchSql',
      'sql': 'SELECT id FROM Entry',
      'parameters': [],
    });
    expect(sql, [
      [
        {'id': 'a'},
      ],
      [
        {'id': 'c'},
      ],
    ]);
  });

  test('each listen creates an independent receiver', () async {
    final (client, carrier) = await fixture(
      initial: full(1, [
        {'id': 'canonical'},
      ]),
    );
    addTearDown(client.close);
    final stream = client.watch('Entry');
    final first = <List<Map<String, dynamic>>>[],
        second = <List<Map<String, dynamic>>>[];
    final a = stream.listen(first.add), b = stream.listen(second.add);
    addTearDown(a.cancel);
    addTearDown(b.cancel);
    await turn();
    expect((carrier.commands.first['spec'] as Map)['delivery'], 'splice-v1');
    first.first.single['id'] = 'mutated';
    carrier.publish([
      changed(
        '1',
        patch(1, [
          splice(1, 0, [
            {'id': 'a'},
          ]),
        ]),
      ),
      changed(
        '2',
        patch(1, [
          splice(1, 0, [
            {'id': 'b'},
          ]),
        ]),
      ),
    ]);
    await turn();
    expect(first.last, [
      {'id': 'canonical'},
      {'id': 'a'},
    ]);
    expect(second.last, [
      {'id': 'canonical'},
      {'id': 'b'},
    ]);
  });

  test(
    'decode failure rejects the whole patch and resumes after full reset',
    () async {
      final (client, carrier) = await fixture(
        initial: full(1, [
          {'id': 'a'},
          {'id': 'b'},
        ]),
      );
      addTearDown(client.close);
      final seen = <List<String>>[], errors = <Object>[];
      final Stream<List<String>> stream = client.watchDecoded<String>(
        'Entry',
        decode: (Map<String, dynamic> row) {
          if (row['id'] == 'bad') throw StateError('decode failed');
          return row['id'] as String;
        },
        copy: (String value) => value,
      );
      final watching = stream.listen(seen.add, onError: errors.add);
      addTearDown(watching.cancel);
      await turn();
      carrier.publish([
        changed(
          '1',
          patch(1, [
            splice(0, 1, [
              {'id': 'partial'},
            ]),
            splice(1, 1, [
              {'id': 'bad'},
            ]),
          ]),
        ),
      ]);
      await turn();
      expect(errors, hasLength(1));
      expect(seen, [
        ['a', 'b'],
      ]);
      carrier.publish([
        changed(
          '1',
          full(3, [
            {'id': 'a'},
            {'id': 'b'},
          ]),
        ),
        changed(
          '1',
          patch(3, [
            splice(1, 1, [
              {'id': 'c'},
            ]),
          ]),
        ),
      ]);
      await turn();
      expect(seen.last, ['a', 'c']);
    },
  );

  test('listener exceptions do not request recovery', () async {
    final (client, carrier) = await fixture(
      initial: full(1, [
        {'id': 'a'},
      ]),
    );
    addTearDown(client.close);
    final failures = <Object>[], seen = <String>[];
    StreamSubscription<List<Map<String, dynamic>>>? subscription;
    runZonedGuarded(() {
      subscription = client.watch('Entry').listen((rows) {
        seen.add(rows.single['id'] as String);
        if (seen.length == 1) throw StateError('listener failed');
      });
    }, (error, stack) => failures.add(error));
    addTearDown(() => subscription?.cancel());
    await turn();
    expect((carrier.commands.first['spec'] as Map)['delivery'], 'splice-v1');
    carrier.publish([
      changed(
        '1',
        patch(1, [
          splice(0, 1, [
            {'id': 'b'},
          ]),
        ]),
      ),
    ]);
    await turn();
    expect(failures, hasLength(1));
    expect(seen, ['a', 'b']);
    expect(
      carrier.commands.where((c) => c['kind'] == 'watchSnapshot'),
      isEmpty,
    );
  });

  test(
    'cancellation before registration and terminal close discard later events',
    () async {
      final (client, carrier) = await fixture(delayed: true);
      addTearDown(client.close);
      final seen = <List<Map<String, dynamic>>>[], errors = <Object>[];
      final watching = client
          .watch('Entry')
          .listen(seen.add, onError: errors.add);
      final cancelled = watching.cancel();
      final request = carrier.admitted.first['requestId'] as String;
      carrier.publish([
        completed(request, {'observerId': '1'}),
        changed(
          '1',
          full(1, [
            {'id': 'late'},
          ]),
        ),
      ]);
      await cancelled;
      await turn();
      expect(seen, isEmpty);
      expect(
        carrier.commands.where((c) => c['kind'] == 'unwatch'),
        hasLength(1),
      );
      final done = Completer<void>();
      client
          .watch('Entry')
          .listen(seen.add, onError: errors.add, onDone: done.complete);
      final next = carrier.admitted.last['requestId'] as String;
      carrier.publish([
        completed(next, {'observerId': '2'}),
        changed(
          '2',
          full(1, [
            {'id': 'a'},
          ]),
        ),
      ]);
      await turn();
      carrier.publish([
        changed('2', {'kind': 'watch', 'closed': true}),
        changed('2', patch(9, [])),
      ]);
      await done.future;
      expect(seen, [
        [
          {'id': 'a'},
        ],
      ]);
      expect(errors, isEmpty);
      expect(
        carrier.commands.where((c) => c['kind'] == 'watchSnapshot'),
        isEmpty,
      );
    },
  );
  for (final failing in ['decode', 'copy']) {
    test(
      'persistent $failing failure reports without a snapshot loop',
      () async {
        final (client, carrier) = await fixture(
          initial: full(1, [
            {'id': 'a'},
          ]),
        );
        addTearDown(client.close);
        var broken = true;
        final seen = <List<String>>[], errors = <Object>[];
        final watching = client
            .watchDecoded<String>(
              'Entry',
              decode: (row) {
                if (broken && failing == 'decode')
                  throw StateError('decode failed');
                return row['id'] as String;
              },
              copy: (value) {
                if (broken && failing == 'copy')
                  throw StateError('copy failed');
                return value;
              },
            )
            .listen(seen.add, onError: errors.add);
        addTearDown(watching.cancel);
        await turn();
        carrier.publish([
          changed(
            '1',
            full(2, [
              {'id': 'a'},
            ]),
          ),
          changed(
            '1',
            patch(2, [
              splice(0, 1, [
                {'id': 'ignored'},
              ]),
            ]),
          ),
        ]);
        await turn();
        expect(errors, hasLength(2));
        expect(seen, isEmpty);
        expect(
          carrier.commands.where((c) => c['kind'] == 'watchSnapshot'),
          hasLength(1),
        );
        broken = false;
        carrier.publish([
          changed(
            '1',
            full(3, [
              {'id': 'reset'},
            ]),
          ),
          changed(
            '1',
            patch(3, [
              splice(0, 1, [
                {'id': 'resumed'},
              ]),
            ]),
          ),
        ]);
        await turn();
        expect(seen, [
          ['reset'],
          ['resumed'],
        ]);
      },
    );
  }

  test(
    'receiver rejection preserves canonical rows and version before recovery',
    () {
      final receiver = ModelWatchReceiver<String>((row) {
        if (row['id'] == 'bad') throw StateError('decode failed');
        return row['id'] as String;
      }, (value) => value);
      final first = receiver.receive(
        full(1, [
          {'id': 'a'},
          {'id': 'b'},
        ]),
      );
      expect(
        () => receiver.receive(
          patch(1, [
            splice(0, 1, [
              {'id': 'partial'},
            ]),
            splice(5, 0),
          ]),
        ),
        throwsStateError,
      );
      expect(
        () => receiver.receive(
          patch(1, [
            splice(0, 1, [
              {'id': 'partial'},
            ]),
            splice(1, 1, [
              {'id': 'bad'},
            ]),
          ]),
        ),
        throwsStateError,
      );
      expect(
        () => receiver.receive(
          full(1, [
            {'id': 'stale'},
          ]),
        ),
        throwsStateError,
      );
      expect(
        receiver.receive(
          patch(1, [
            splice(1, 1, [
              {'id': 'c'},
            ]),
          ]),
        ),
        ['a', 'c'],
      );
      expect(first, ['a', 'b']);
    },
  );
  test('receiver replays multirow insertion with sequential deletion', () {
    var decoded = 0;
    final receiver = ModelWatchReceiver<String>((row) {
      decoded++;
      return row['id'] as String;
    }, (value) => value);
    receiver.receive(
      full(1, [
        {'id': 'a'},
        {'id': 'b'},
      ]),
    );
    expect(
      receiver.receive(
        patch(1, [
          splice(1, 0, [
            {'id': 'x'},
            {'id': 'y'},
            {'id': 'z'},
          ]),
          splice(4, 1),
        ]),
      ),
      ['a', 'x', 'y', 'z'],
    );
    expect(decoded, 5);
  });
}
