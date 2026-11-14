// Authored empty protocol carriers for transport tests, and a one-shot Stream
// delivery that seeds delivered rows. No publication, Loader or settlement
// decision is implemented by this fixture.
import 'dart:convert';
import 'dart:io';
import 'package:axton/axton.dart';
import 'package:crypto/crypto.dart';
import 'store_fixture.dart';

Map<String, Object?> contextOf(Map body) => {
  for (final key in ['protocol', 'storeId', 'stream', 'materialization'])
    key: body[key],
};
String _domainHash(String domain, Object value) => sha256.convert([
  ...utf8.encode(domain),
  0,
  ...utf8.encode(_canonical(value)),
]).toString();
Map<String, Object?> emptyHandshake(Map body) => {
  'protocol': 6,
  'storeId': body['storeId'],
  'stream': body['stream'],
  'head': 0,
  'projectionGeneration': '1',
};
Map<String, Object?> deliveryPage(Map body, [List<Object?> changes = const []]) {
  final unit = {'index': 0, 'through': body['through'], 'changes': changes};
  final manifest = {
    'through': body['through'],
    'minimumCursor': changes.isEmpty
        ? null
        : changes
              .map((c) => (c as Map)['cursor'] as int)
              .reduce((a, b) => a < b ? a : b),
    'digest': _domainHash('axton:delivery-unit:5', unit),
    'parts': [
      _domainHash('axton:delivery-part:5', {
        'unit': 0,
        'part': 0,
        'changes': changes,
      }),
    ],
  };
  final header = <String, Object?>{
    ...contextOf(body),
    'planId': 'transport-${body['storeId']}',
    'bootstrap': body['bootstrap'],
    'after': body['after'],
    'through': body['through'],
    'observedHead': body['through'],
    'expiresAt': DateTime.now().millisecondsSinceEpoch + 300000,
    'units': [manifest],
  };
  header['digest'] = _domainHash('axton:delivery-plan:5', header);
  return {
    'header': header,
    'parts': [
      {
        'planId': header['planId'],
        'planDigest': header['digest'],
        'unit': 0,
        'part': 0,
        'changes': changes,
      },
    ],
  };
}

Map<String, Object?> readResponse(Map body, Object? result, List<Object?> records) =>
    {
      ...contextOf(body),
      'requestId': body['requestId'],
      'outcome': {'kind': 'succeeded', 'result': result},
      'records': records,
    };

Map<String, Object?> emptyPull(Map body) => body.containsKey('materialization')
    ? deliveryPage(body)
    : emptyHandshake(body);

Map<String, Object?> emptyRead(Map body) => readResponse(
  body,
  null,
  body['invocation']['kind'] == 'fetch'
      ? [
          {'key': body['invocation']['key'], 'cursor': null, 'state': null},
        ]
      : [],
);

String _canonical(Object? value) {
  if (value is Map) {
    final keys = value.keys.cast<String>().toList()..sort();
    return '{${keys.map((key) => '${jsonEncode(key)}:${_canonical(value[key])}').join(',')}}';
  }
  if (value is List) return '[${value.map(_canonical).join(',')}]';
  return jsonEncode(value);
}

Map<String, Object?> emptyMutation(Map body) => {
  ...contextOf(body),
  'batchId': body['batchId'],
  'digest': body['digest'],
  'results': [
    for (final mutation in body['mutations'])
      {
        'mutationId': mutation['id'],
        'outcome': {
          'kind': 'accepted',
          'syncCursor': 0,
          'materialization': body['materialization'],
          'result': null,
          'targets': <Object>[],
        },
      },
  ],
};

Future<bool> answerEmptyBackground(HttpRequest request) async {
  if (request.uri.path == '/sync/pull' ||
      request.uri.path == '/sync/handshake') {
    final body = jsonDecode(await utf8.decoder.bind(request).join()) as Map;
    request.response.write(jsonEncode(emptyPull(body)));
    await request.response.close();
    return true;
  }
  if (request.uri.path == '/sync/live') {
    final socket = await WebSocketTransformer.upgrade(request);
    socket.listen((message) {
      final body = jsonDecode(message as String) as Map;
      socket.add(jsonEncode(emptyHandshake(body)));
    }, onError: (Object _) {});
    return true;
  }
  return false;
}

/// Install [records] in [client]'s Store as delivered rows (evidenced, not
/// pending): a throwaway Stream server announces them in its handshake and
/// the connection's automatic Bootstrap pulls them. Each record is
/// `{model, identity, state}`; they take the cursors after [after], the
/// last cursor an earlier call delivered to this Store. A client that had a connection is connected
/// to [offline] again on return ([offlineStoreConnection] when null), as the
/// tests that use this opened it; one without stays without.
Future<void> deliverRecords(
  Client client,
  List<Map<String, Object?>> records, {
  int after = 0,
  StoreConnection? offline,
}) async {
  final changes = [
    for (final (index, record) in records.indexed)
      <String, Object?>{
        'kind': 'record',
        'cursor': after + index + 1,
        'key': {'model': record['model'], 'identity': record['identity']},
        'state': record['state'],
      },
  ];
  Map<String, Object?> handshake(Map body) => {
    ...emptyHandshake(body),
    'head': after + changes.length,
  };
  final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
  server.listen((request) async {
    if (request.uri.path == '/sync/live') {
      final socket = await WebSocketTransformer.upgrade(request);
      socket.listen((message) {
        socket.add(jsonEncode(handshake(jsonDecode(message as String) as Map)));
      }, onError: (Object _) {});
      return;
    }
    final body = jsonDecode(await utf8.decoder.bind(request).join()) as Map;
    request.response.write(
      jsonEncode(switch (request.uri.path) {
        '/sync/handshake' => handshake(body),
        '/sync/pull' => deliveryPage(body, [
          for (final change in changes)
            if ((change['cursor'] as int) > (body['after'] as int) &&
                (change['cursor'] as int) <= (body['through'] as int))
              change,
        ]),
        _ => emptyRead(body),
      }),
    );
    await request.response.close();
  });
  final wasConnected = client.connection != null;
  try {
    await client.connection?.close();
    await client.connect(
      SyncServer(url: 'http://127.0.0.1:${server.port}', token: () => 'seed'),
      onError: (_) {},
    );
    final deadline = DateTime.now().add(const Duration(seconds: 5));
    for (final record in records) {
      bool delivered(Map<String, dynamic>? row) =>
          row != null &&
          (record['state'] as Map).entries.every(
            (field) => jsonEncode(row[field.key]) == jsonEncode(field.value),
          );
      while (!delivered(
        await client.read(
          record['model'] as String,
          (record['identity'] as Map).cast<String, dynamic>(),
        ),
      )) {
        if (DateTime.now().isAfter(deadline)) {
          throw StateError('record ${record['identity']} was not delivered');
        }
        await Future<void>.delayed(const Duration(milliseconds: 5));
      }
    }
    await client.connection?.close();
  } finally {
    await server.close(force: true);
  }
  if (!wasConnected) return;
  final again = offline ?? offlineStoreConnection();
  await client.connect(
    again,
    onError: again.onError ?? (_) {},
    refreshAuth: again.refreshAuth,
    directTimeout: again.directTimeout,
  );
}
