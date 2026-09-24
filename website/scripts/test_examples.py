"""Keep snippet verification active for code inside language tabs."""
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from check_examples import snippets


class SnippetTests(unittest.TestCase):
    def test_extracts_each_language_without_swallowing_prose(self):
        markdown = '''# Read a record

=== "TypeScript"

    ```ts
    import { Client } from './client.ts';
    await client.transaction(async tx => {
      await tx.read('Entry', { id: 'entry-1' });
    });
    ```

=== "Flutter"

    ```dart
    final entry = await client.models.entry.get(id);
    ```

## More details

Shared explanation.

```ts
await client.close();
```
'''
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / 'guide.md').write_text(markdown)
            with patch('check_examples.ROOT', root):
                ts = snippets('ts', ['guide.md'])
                dart = snippets('dart', ['guide.md'])
        self.assertEqual([code for _, code in ts], [
            "await client.transaction(async tx => {\n  await tx.read('Entry', { id: 'entry-1' });\n});\n",
            'await client.close();\n',
        ])
        self.assertEqual(dart, [('guide.md:14', 'final entry = await client.models.entry.get(id);\n')])

    def test_routes_typescript_alias_and_action_contract_marker(self):
        markdown = '''```ts
await client.close();
```
```typescript
await client.transaction(async tx => {});
```
```typescript title="action-contract"
await client.actions.call.getTodos({});
```
```ts title="action-contract"
await client.actions.call.sendEmail({ to: 'a', subject: 'b', body: 'c' });
```
'''
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / 'guide.md').write_text(markdown)
            with patch('check_examples.ROOT', root):
                ordinary = snippets('ts', ['guide.md'])
                action = snippets('ts', ['guide.md'], context='action')
        self.assertEqual([code for _, code in ordinary], [
            'await client.close();\n',
            'await client.transaction(async tx => {});\n',
        ])
        self.assertEqual([code for _, code in action], [
            'await client.actions.call.getTodos({});\n',
            "await client.actions.call.sendEmail({ to: 'a', subject: 'b', body: 'c' });\n",
        ])

    def test_routes_dart_action_snippets_to_generated_action_fixture(self):
        markdown = '''```dart
await client.close();
```
```dart title="action-contract"
final result = await client.actions.call.getTodos();
```
'''
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / 'guide.md').write_text(markdown)
            with patch('check_examples.ROOT', root):
                ordinary = snippets('dart', ['guide.md'])
                action = snippets('dart', ['guide.md'], context='action')
        self.assertEqual([code for _, code in ordinary], ['await client.close();\n'])
        self.assertEqual([code for _, code in action],
                         ['final result = await client.actions.call.getTodos();\n'])


if __name__ == '__main__':
    unittest.main()
