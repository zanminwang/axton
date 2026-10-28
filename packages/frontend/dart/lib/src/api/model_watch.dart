/// Copy JSON containers rather than retaining application-owned mutable values.
Map<String, dynamic> copyWatchRecord(Map<String, dynamic> row) =>
    _copyJson(row) as Map<String, dynamic>;

dynamic _copyJson(dynamic value) {
  if (value is List) return [for (final item in value) _copyJson(item)];
  if (value is Map) {
    return <String, dynamic>{
      for (final entry in value.entries)
        entry.key as String: _copyJson(entry.value),
    };
  }
  return value;
}

bool _integer(dynamic value, int minimum) =>
    value is int && value >= minimum && value <= 9007199254740991;

/// Rust owns membership/order; this cache only replays sequential splices.
class ModelWatchReceiver<T> {
  ModelWatchReceiver(this._decode, this._copy);
  final T Function(Map<String, dynamic>) _decode;
  final T Function(T) _copy;
  List<T> _rows = [];
  int? _version;
  bool _recovering = false;

  /// Request at most one snapshot until a full reset arrives.
  bool recover() {
    if (_recovering) return false;
    _recovering = true;
    return true;
  }

  List<T>? receive(Map<String, dynamic> snapshot) {
    if (snapshot['closed'] == true) return null;
    if (_recovering && !snapshot.containsKey('rows')) return null;
    StateError invalid() => StateError('invalid Model watch snapshot');
    if (snapshot['kind'] != 'watch') throw invalid();
    int? version;
    List<T> next;
    if (!snapshot.containsKey('protocol')) {
      if (snapshot.containsKey('splices') ||
          snapshot.containsKey('version') ||
          snapshot.containsKey('baseVersion')) {
        throw invalid();
      }
      next = _decodeRows(snapshot['rows']);
    } else {
      if (snapshot['protocol'] != 'splice-v1' ||
          !_integer(snapshot['version'], 1)) {
        throw invalid();
      }
      version = snapshot['version'] as int;
      if (snapshot.containsKey('rows')) {
        if (snapshot.containsKey('splices') ||
            snapshot.containsKey('baseVersion') ||
            (_version != null && version <= _version!)) {
          throw invalid();
        }
        next = _decodeRows(snapshot['rows']);
      } else {
        if (_version == null ||
            !_integer(snapshot['baseVersion'], 1) ||
            snapshot['baseVersion'] != _version ||
            version != snapshot['baseVersion'] + 1 ||
            snapshot['splices'] is! List) {
          throw invalid();
        }
        next = List<T>.of(_rows);
        for (final splice in snapshot['splices'] as List) {
          if (splice is! Map ||
              !_integer(splice['index'], 0) ||
              !_integer(splice['deleteCount'], 0)) {
            throw invalid();
          }
          final index = splice['index'] as int;
          final deleteCount = splice['deleteCount'] as int;
          if (index > next.length || deleteCount > next.length - index) {
            throw invalid();
          }
          final inserted = _decodeRows(splice['rows']);
          next.replaceRange(index, index + deleteCount, inserted);
        }
      }
    }
    // Decode/copy the complete delivery before changing the private cache.
    final delivered = [for (final value in next) _copy(value)];
    _rows = next;
    _version = version;
    _recovering = false;
    return delivered;
  }

  List<T> _decodeRows(dynamic rows) {
    if (rows is! List || rows.any((row) => row is! Map<String, dynamic>)) {
      throw StateError('invalid Model watch rows');
    }
    return [for (final row in rows) _decode(row as Map<String, dynamic>)];
  }
}
