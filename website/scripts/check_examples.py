"""Typecheck client snippets directly from the maintained Markdown sources.

Run after scripts/build.sh, npm ci at the root, and Dart package resolution.
The real HTTP/SQLite behavior is covered by integration/e2e/run.sh.
"""
from pathlib import Path
import re
import subprocess
import tempfile
import textwrap

ROOT = Path(__file__).resolve().parents[2]
SOURCES = {
    'ts': ['website/docs/frontend/client-api.md', 'website/docs/frontend/runtime.md',
           'website/docs/frontend/setup.md', 'website/docs/frontend/sync.md'],
    'dart': ['website/docs/frontend/client-api.md', 'website/docs/frontend/runtime.md',
             'website/docs/frontend/setup.md', 'website/docs/frontend/sync.md'],
}
BACKEND_SOURCES = ['website/docs/backend/api.md', 'website/docs/backend/database.md',
                   'website/docs/backend/setup.md']


def snippets(language, sources=None):
    result = []
    for source in sources if sources is not None else SOURCES[language]:
        text = (ROOT / source).read_text()
        for match in re.finditer(r'^(?P<indent> *)```' + language + r'\n(.*?)^(?P=indent)```', text, re.M | re.S):
            code = re.sub(r'^import .*?;\n', '', textwrap.dedent(match[2]), flags=re.M | re.S)
            line = text[:match.start()].count('\n') + 1
            result.append((f'{source}:{line}', code))
    return result


def check():
    # Keep temporary sources within package ancestry so module resolution uses
    # the same installed dependencies and Dart package config as the fixture.
    with tempfile.TemporaryDirectory(prefix='.docs-check-', dir=ROOT / 'packages/dart') as temp:
        directory = Path(temp)
        ts = directory / 'examples.mts'
        ts.write_text('''import { GeneratedClient, Edit, schema } from '../../../integration/e2e/fixtures/round-trip/generated/client.ts';
import type { Transaction } from '../../client-js/index.mts';
declare const client: GeneratedClient;
declare const backendUrl: string;
declare let accessToken: string;
declare function renewAccessToken(): Promise<string>;
declare function uploadFile(key: unknown): Promise<void>;
declare function render(entries: unknown): void;
declare const rejectionOrdinal: number;
declare const stop: () => void;
''' + '\n'.join(f'// {source}\nasync function example{i}() {{\n{code}\n}}'
                  for i, (source, code) in enumerate(snippets('ts'))))
        subprocess.run([str(ROOT / 'node_modules/.bin/tsc'), '--noEmit', '--strict',
                        '--exactOptionalPropertyTypes', '--skipLibCheck', '--target', 'ES2022',
                        '--module', 'NodeNext', '--moduleResolution', 'NodeNext',
                        '--allowImportingTsExtensions', str(ts)], cwd=ROOT, check=True)
        dart = directory / 'examples.dart'
        dart.write_text('''// ignore_for_file: unused_local_variable, unused_import
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:axton/axton.dart';
import '../../../integration/e2e/fixtures/round-trip/generated/generated.dart';
late GeneratedClient client;
late StreamSubscription<List<Entry>> subscription;
late HttpClient http;
void render(List<Entry> entries) {}
const rejectionOrdinal = 1;
late String accessToken;
late String backendUrl;
Future<String> renewAccessToken() async => '';
Future<void> uploadFile(dynamic key) async {}
''' + '\n'.join(f'// {source}\nFuture<void> example{i}() async {{\n{code}\n}}'
                  for i, (source, code) in enumerate(snippets('dart'))))
        subprocess.run(['dart', 'analyze', str(dart)], cwd=ROOT / 'packages/dart', check=True)
    with tempfile.TemporaryDirectory(prefix='.docs-check-', dir=ROOT / 'integration/e2e/fixtures/round-trip') as temp:
        backend = Path(temp) / 'backend.mts'
        backend.write_text('''import { PrismaClient, type Prisma } from '@prisma/client';
import { createBackend, devAuth, Entry, MutationRejected, type Handlers, type Loaders } from '../generated/backend.ts';
import { prisma } from '../../../../../packages/postgres/index.mts';
import type { Database, Publish } from '../../../../../packages/server/index.mts';
type Tx = Prisma.TransactionClient;
declare function canEdit(tx: Tx, userId: string, identity: { id: string }): Promise<boolean>;
declare function loadVisibleEntry(tx: Tx, userId: string, identity: { id: string }): Promise<Entry | null>;
declare const changes: ReturnType<typeof Entry>[];
declare const db: PrismaClient;
declare const database: Database<Prisma.TransactionClient>;
declare const handlers: Handlers<Prisma.TransactionClient>;
declare const loaders: Loaders<Prisma.TransactionClient>;
declare const backend: ReturnType<typeof createBackend<Prisma.TransactionClient>>;
declare const publish: Publish;
''' + '\n'.join(f'// {source}\nasync function example{i}() {{\n{code.replace("export const", "const")}\n}}'
                  for i, (source, code) in enumerate(snippets('ts', BACKEND_SOURCES))))
        subprocess.run([str(ROOT / 'node_modules/.bin/tsc'), '--noEmit', '--strict',
                        '--exactOptionalPropertyTypes', '--skipLibCheck', '--target', 'ES2022',
                        '--module', 'NodeNext', '--moduleResolution', 'NodeNext',
                        '--allowImportingTsExtensions', str(backend)], cwd=ROOT, check=True)
    with tempfile.TemporaryDirectory(prefix='axton-docs-schema-') as temp:
        for i, (source, code) in enumerate(snippets('text', ['website/docs/schema/define.md'])):
            directory = Path(temp) / str(i)
            directory.mkdir()
            (directory / 'example.model').write_text(code)
            subprocess.run([str(ROOT / 'target/debug/axton'), 'compile', str(directory),
                            str(directory / 'generated')], cwd=ROOT, check=True)
            print(f'Compiled schema from {source}')
    print(f"Typechecked {len(snippets('ts')) + len(snippets('ts', BACKEND_SOURCES))} TypeScript and {len(snippets('dart'))} Dart documentation snippets.")


if __name__ == '__main__':
    check()
