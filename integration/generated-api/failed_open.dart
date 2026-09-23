import 'dart:io';
import 'generated.dart';
Future<void> main(List<String> args) async {
 try {
  await GeneratedClient.open(path:args[0],libraryPath:Platform.environment['AXTON_DART_LIBRARY'],server:SyncServer(url:'http://[',token:()=>'secret'));
  throw StateError('invalid URL unexpectedly opened');
 } on FormatException {
  if (File('${args[0]}-wal').existsSync() || File('${args[0]}-shm').existsSync()) {
   throw StateError('failed open left native SQLite connection alive');
  }
 }
}
