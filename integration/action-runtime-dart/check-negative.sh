#!/usr/bin/env bash
set -uo pipefail
cd "$(dirname "$0")"
output="$(dart analyze negative.dart 2>&1)"
status=$?
if [[ $status -eq 0 ]]; then
  echo 'Generated Dart misuse analyzed cleanly' >&2
  exit 1
fi
expected=(
  'argument_type_not_assignable:7'
  'list_element_type_not_assignable:10'
  'undefined_named_parameter:15'
  'undefined_method:18'
  'undefined_getter:20'
  'undefined_getter:21'
  'invalid_assignment:22'
  'undefined_getter:24'
)
for pair in "${expected[@]}"; do
  code="${pair%%:*}"
  line="${pair##*:}"
  if ! grep -E "negative\.dart:${line}:[0-9]+ - .* - ${code}$" <<<"$output" >/dev/null; then
    echo "Expected $code at line $line" >&2
    echo "$output" >&2
    exit 1
  fi
done
echo "Generated Dart misuse refused: ${#expected[@]} expected analyzer errors."
