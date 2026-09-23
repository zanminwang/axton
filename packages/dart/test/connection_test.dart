import 'dart:async';
import 'package:axton/axton.dart';
import 'package:test/test.dart';

void main() {
  test('wake while idle decision is in flight is retained', () async {
    final gate = Completer<void>();
    var calls = 0;
    final connection = await RuntimeConnection.start(
      control: (event, now, entropy) async {
        if (event == 'next') {
          if (++calls == 1) await gate.future;
          return {'type': 'idle'};
        }
        return null;
      },
      sync: (_) async {},
      transport: (_, __) async => '',
    );
    await connection.wake();
    gate.complete();
    await Future<void>.delayed(const Duration(milliseconds: 10));
    expect(calls, greaterThanOrEqualTo(2));
    await connection.close();
  });
  test('close abandons a transport which never resolves', () async {
    final entered = Completer<void>();
    final never = Completer<String>();
    final events = <String>[];
    final connection = await RuntimeConnection.start(
      control: (event, now, entropy) async {
        events.add(event);
        return {'type': 'sync'};
      },
      sync: (request) async {
        entered.complete();
        await request('push', '{}');
      },
      transport: (_, __) => never.future,
    );
    await entered.future;
    await connection.close();
    await Future<void>.delayed(Duration.zero);
    expect(events, contains('stop'));
    expect(events, isNot(contains('success')));
    expect(events, isNot(contains('failure')));
  });
  test('closed controls cannot alter a replacement driver', () async {
    final events = <String>[];
    final connection = await RuntimeConnection.start(
      control: (event, now, entropy) async {
        events.add(event);
        return {'type': 'idle'};
      },
      sync: (_) async {},
      transport: (_, __) async => '',
    );
    await connection.close();
    final ended = events.length;
    await connection.pause();
    await connection.resume();
    await connection.wake();
    await connection.close();
    expect(events.length, ended);
  });
}
