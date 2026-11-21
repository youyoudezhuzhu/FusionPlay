/// All reads and commands in a browser tab are bound to the URL that opened it.
/// In particular, a stale bookmark must never send commands to another room.
class SpeakerPlayerLocation {
  SpeakerPlayerLocation(this.page);

  final Uri page;
  static final _validId = RegExp(r'^[a-z0-9-]{1,40}$');

  bool get pinned => page.queryParameters.containsKey('instance');
  String get id => page.queryParameters['instance'] ?? '';

  SpeakerPlayerLocation resolveDefault(
    String? preferred,
    Iterable<String> available,
  ) {
    if (pinned ||
        preferred == null ||
        !_validId.hasMatch(preferred) ||
        !available.contains(preferred)) {
      return this;
    }
    return SpeakerPlayerLocation(select(preferred));
  }

  Uri get root => page.resolve('../');
  Uri get catalog => root.resolve('api/instances');

  Uri api(String resource) {
    if (!_validId.hasMatch(id)) throw const FormatException('无效的音箱标识');
    return root.resolve('api/instances/$id/$resource');
  }

  Uri? artwork(String? source) {
    if (source == null || source.isEmpty) return null;
    return source.startsWith('artwork/') ? api(source) : root.resolve(source);
  }

  Uri get settings => root
      .resolve('index.html')
      .replace(
        queryParameters: pinned ? {'instance': id} : null,
        fragment: '/settings',
      );

  Uri speakerPage(String section) => root
      .resolve('speakers.html')
      .replace(
        queryParameters: pinned ? {'instance': id} : null,
        fragment: section,
      );

  Uri get speakerPicker => speakerPage('rooms');
  Uri get createInstance => speakerPage('new');

  Uri select(String nextId) => page.replace(
    queryParameters: {...page.queryParameters, 'instance': nextId},
  );
}
