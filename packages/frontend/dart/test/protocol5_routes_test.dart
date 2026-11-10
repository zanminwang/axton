import 'dart:convert';
import 'dart:io';
import 'package:axton/src/bindings/connection.dart';
import 'package:axton/src/bindings/live.dart';
import 'package:test/test.dart';
import 'connection_test.dart' show FakeHost;

void main() {
  test(
    'protocol5 HTTP effects preserve route and body and refuse others',
    () async {
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      final seen = <List<String>>[];
      server.listen((request) async {
        seen.add([
          request.uri.path,
          await utf8.decoder.bind(request).join(),
          request.headers.value('authorization')!,
        ]);
        request.response.write('native answer');
        await request.response.close();
      });
      final host = FakeHost();
      final connection = await RuntimeConnection.connect(
        host: host,
        network: ServerSession(
          SyncServer(
            url: 'http://127.0.0.1:${server.port}',
            token: () => 'credential',
          ),
        ),
      );
      try {
        for (final route in ['handshake', 'push', 'pull', 'action', 'fetch']) {
          final id = host.effect({
            'kind': 'http',
            'route': route,
            'body': 'frozen',
          });
          final outcome = await host.answerOf(id);
          expect(outcome['ok'], true, reason: outcome.toString());
          expect(outcome['value'], 'native answer');
        }
        final refused = await host.answerOf(
          host.effect({'kind': 'http', 'route': 'load', 'body': 'frozen'}),
        );
        expect(refused['ok'], false);
        expect(seen, [
          ['/sync/handshake', 'frozen', 'Bearer credential'],
          ['/sync/mutations', 'frozen', 'Bearer credential'],
          ['/sync/pull', 'frozen', 'Bearer credential'],
          ['/sync/actions', 'frozen', 'Bearer credential'],
          ['/sync/fetch', 'frozen', 'Bearer credential'],
        ]);
      } finally {
        await connection.close();
        await server.close(force: true);
      }
    },
  );
}
