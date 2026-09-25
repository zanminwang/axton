import 'dart:io';
import 'dart:convert';
import 'package:test/test.dart';
import 'generated.dart';
void main(){
 test('failed generated open closes its native worker isolate',()async{
  final temp=await Directory.systemTemp.createTemp('generated-failed-open-');
  final child=await Process.start(Platform.resolvedExecutable,['failed_open.dart','${temp.path}/state.sqlite']);
  try{final code=await child.exitCode.timeout(const Duration(seconds:3));expect(code,0,reason:await child.stderr.transform(utf8.decoder).join());}
  finally{child.kill();await temp.delete(recursive:true);}
 });
 const id='123e4567-e89b-42d3-a456-426614174000';
 final row=Entry(id:id,title:'hello',note:null,at:DateTime.utc(2026),tags:['x'],status:Status.active);
 test('source conversion, patch absence and explicit null',(){
  expect(Entry.fromRecord(row.toRecord()).at,row.at);
  expect(const EntryPatch(note:Present(null)).toRecord(),{'note':null});
  expect(const EntryPatch().toRecord(),isEmpty);
  expect((createEntry(entry:row)['operations'] as List).single['values'].containsKey('id'),false);
  expect((removeEntries(entries:[])['operations'] as List),isEmpty);
 });
 test('generated mutations and query use real native client',()async{
  final temp=await Directory.systemTemp.createTemp('generated-api-');
  final client=await GeneratedClient.open(path:'${temp.path}/state.sqlite',libraryPath:Platform.environment['AXTON_DART_LIBRARY'] ?? '../../target/debug/libaxton_dart.dylib');
  try{
   expect(await client.mutate.createEntry(entry:row),1);
   expect((await client.models.entry.get(const EntryIdentity(id:id)))?.title,'hello');
   await client.mutate.editEntry(entry:const EditEntryEntryUpdate(identity:EntryIdentity(id:id),note:Present('changed')));
   await client.mutate.editEntry(entry:const EditEntryEntryUpdate(identity:EntryIdentity(id:id),note:Present(null)));
   expect((await client.models.entry.get(const EntryIdentity(id:id)))?.note,isNull,reason:'independent mutations apply locally in order');
   final loaded=(await client.models.entry.query()).single;
   expect(loaded.note,isNull);expect(loaded.title,'hello');expect(loaded.at,row.at);
   expect((await client.models.entry.query(where:EntryFilter(at:Present(DateTime.parse('2026-01-01T01:00:00+01:00')),note:const Present(null)),orderBy:const [EntryOrder(EntryOrderField.byTitle,descending:true)],limit:1)).length,1);
   await client.mutate.addBook(book:const Book(id:'b',title:'Book'));
   await client.mutate.addComment(comment:const Comment(id:'c',bookId:'b',text:'Comment'));
   expect((await client.models.comment.book(const CommentIdentity(id:'c')))?.id,'b');
   expect((await client.models.book.comments(const BookIdentity(id:'b'))).length,1);
   await client.transaction((tx)async{
    await tx.models.book.create(const Book(id:'local',title:'Local only'));
    await tx.models.book.update(const BookIdentity(id:'local'),const BookPatch(title:Present('Local edited')));
   });
   expect((await client.models.book.get(const BookIdentity(id:'local')))?.title,'Local edited');
   // A mutation outside a transaction is its own transaction; its record's sync state is typed.
   final ordinal=await client.mutate.editEntry(entry:const EditEntryEntryUpdate(identity:EntryIdentity(id:id),note:Present('outside')));
   final state=await client.models.entry.syncState(const EntryIdentity(id:id));
   expect(state.pending.map((p)=>p.ordinal),contains(ordinal));
   expect(state.pending.every((p)=>p.phase=='queued' && !p.diverged),isTrue);
   expect(state.rejections,isEmpty);
   expect((await client.syncState())['pending'],greaterThan(0));
   expect(client.clientId,isNotEmpty);
   expect(await client.client.freeze(),isNotNull);
  }finally{await client.close();await temp.delete(recursive:true);}
 });
 // The generated Scope facade ([#150](https://github.com/zanminwang/axton/issues/150)):
 // one handle per registration, typed handle members, and the retained
 // `channels` spelling on that same ledger path.
 test('generated scopes facade answers with one handle per registration',()async{
  final temp=await Directory.systemTemp.createTemp('generated-api-scopes-');
  final client=await GeneratedClient.open(path:'${temp.path}/state.sqlite',libraryPath:Platform.environment['AXTON_DART_LIBRARY'] ?? '../../target/debug/libaxton_dart.dylib');
  try{
   final handles=await Future.wait([client.scopes.subscribe('project:123'),client.scopes.subscribe('project:123')]);
   final Subscription a=handles.first;
   expect(identical(a,handles.last),isTrue,reason:'concurrent calls obtain one cached handle');
   expect(a.status.initialization,SubscriptionInitialization.pending);
   await a.unsubscribe();
   final c=await client.scopes.subscribe('project:123');
   await a.unsubscribe();
   expect(c.status.active,isTrue,reason:'an old handle cannot remove the registration that replaced it');
   // The handle is the runtime's: its Scope, its immutable status and its
   // observers are all named through the generated library.
   final String scope=c.scope;
   final SubscriptionStatus status=c.status;
   expect(scope,'project:123');
   expect(status.connection,SubscriptionConnection.offline);
   final seen=<SubscriptionStatus>[];
   final observer=c.watch().listen(seen.add);
   await pumpEventQueue();
   await observer.cancel();
   expect(seen.map((s)=>s.connection),[SubscriptionConnection.offline],reason:'the current snapshot arrives first');
   // The retained spelling registers through the same ledger: with no server it
   // has durable intent and no boundary.
   final Subscription retained=await client.channels.subscribe('project:456');
   expect(retained.status.initialization,SubscriptionInitialization.pending);
   await client.channels.unsubscribe('project:456');
   expect(retained.status.active,isFalse);
   await c.unsubscribe();
  }finally{await client.close();await temp.delete(recursive:true);}
 });
}
