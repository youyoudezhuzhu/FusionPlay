import 'dart:async';
import 'dart:convert';
import 'dart:js_interop';

import 'package:flutter/material.dart';
import '../../models/app_state.dart';
import '../../ui/liquid_glass.dart';
import '../../ui/settings_view.dart';
import 'player_location.dart';

@JS('fpRequest')
external JSPromise<JSString> _request(
  JSString url,
  JSString method,
  JSString? payload,
);

@JS('fpDownloadText')
external void _downloadText(JSString filename, JSString text);

@JS('fpOpenConfig')
external JSPromise<JSString> _openConfig();

@JS('fpBrowserErrors')
external JSString _browserErrors();

Future<Map<String, dynamic>> fnosRequest(
  Uri uri, {
  String method = 'GET',
  Object? body,
}) async {
  final result = await _request(
    uri.toString().toJS,
    method.toJS,
    body == null ? null : jsonEncode(body).toJS,
  ).toDart;
  return jsonDecode(result.toDart) as Map<String, dynamic>;
}

class FnosSettings extends StatefulWidget {
  const FnosSettings({
    super.key,
    required this.location,
    required this.initialFocus,
    required this.advancedEffects,
    required this.onAdvancedEffects,
    required this.onClose,
    required this.onOpenUpdate,
  });

  final SpeakerPlayerLocation location;
  final FocusNode initialFocus;
  final bool advancedEffects;
  final ValueChanged<bool> onAdvancedEffects;
  final VoidCallback onClose;
  final VoidCallback onOpenUpdate;

  @override
  State<FnosSettings> createState() => _FnosSettingsState();
}

class _FnosSettingsState extends State<FnosSettings> {
  Map<String, dynamic>? config;
  String version = fusionPlayVersion;
  String? loadError;
  bool busy = false;

  Uri api(String path) => widget.location.root.resolve('api/$path');

  @override
  void initState() {
    super.initState();
    load();
  }

  Future<void> load() async {
    setState(() => loadError = null);
    try {
      final current = await fnosRequest(api('config'));
      if (mounted) setState(() => config = current);
    } catch (_) {
      if (mounted) setState(() => loadError = '读取飞牛设置失败');
    }
    try {
      final status = await fnosRequest(api('status'));
      if (mounted && status['version'] is String) {
        setState(() => version = status['version'] as String);
      }
    } catch (_) {
      // The packaged version remains available during a receiver restart.
    }
  }

  Future<void> change(Map<String, Object?> fields) async {
    if (busy) return;
    setState(() => busy = true);
    try {
      final response = await fnosRequest(
        api('config'),
        method: 'PATCH',
        body: fields,
      );
      if (mounted) {
        setState(() => config = response['config'] as Map<String, dynamic>);
      }
    } catch (error) {
      feedback('保存设置失败：$error');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  void feedback(String message) {
    if (!mounted) return;
    unawaited(
      showGlassDialog<void>(
        context: context,
        builder: (dialog) => Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(message),
            TextButton(
              onPressed: () => Navigator.pop(dialog),
              child: const Text('关闭'),
            ),
          ],
        ),
      ),
    );
  }

  Future<void> exportLogs() async {
    try {
      final result = await fnosRequest(api('logs/export'));
      final browser = _browserErrors().toDart;
      final report = result['report'] as String;
      _downloadText(
        'FusionPlay-fnOS-logs.txt'.toJS,
        (browser.isEmpty ? report : '$report\n\n浏览器诊断\n$browser').toJS,
      );
    } catch (error) {
      feedback('导出日志失败：$error');
    }
  }

  Future<void> backup() async {
    try {
      final result = await fnosRequest(api('export'));
      final contents = const JsonEncoder.withIndent(
        '  ',
      ).convert(result['config']);
      _downloadText('FusionPlay-fnOS-config.json'.toJS, contents.toJS);
    } catch (error) {
      feedback('导出配置失败：$error');
    }
  }

  Future<void> restore() async {
    try {
      final contents = (await _openConfig().toDart).toDart;
      if (contents.isEmpty || !mounted) return;
      final decoded = jsonDecode(contents);
      final incoming = decoded is Map && decoded['config'] is Map
          ? decoded['config']
          : decoded;
      if (incoming is! Map<String, dynamic> ||
          ![
            'receiver',
            'protocols',
            'audio',
          ].every((key) => incoming[key] is Map)) {
        throw const FormatException('请选择完整的 FusionPlay 飞牛配置文件');
      }
      final confirm = await showGlassDialog<bool>(
        context: context,
        builder: (dialog) => Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Text('导入会替换当前接收器与音箱配置。'),
            Wrap(
              children: [
                TextButton(
                  onPressed: () => Navigator.pop(dialog, false),
                  child: const Text('取消'),
                ),
                TextButton(
                  onPressed: () => Navigator.pop(dialog, true),
                  child: const Text('确认导入'),
                ),
              ],
            ),
          ],
        ),
      );
      if (confirm != true || !mounted) return;
      await fnosRequest(api('import'), method: 'POST', body: incoming);
      if (mounted) await load();
    } catch (error) {
      feedback('导入配置失败：$error');
    }
  }

  Future<void> network() async {
    final receiver = config!['receiver'] as Map;
    final interface = await _editValue(
      context,
      '网络接口',
      receiver['interface'] as String? ?? '',
      hint: '留空时自动选择',
    );
    if (interface != null && mounted) {
      await change({'interface': interface.trim()});
    }
  }

  Future<void> pairing() async {
    final receiver = config!['receiver'] as Map;
    final pin = await _editValue(
      context,
      'AirPlay 配对 PIN',
      receiver['pin'] as String? ?? '',
      hint: '留空时取消固定 PIN',
      obscureText: true,
    );
    if (pin != null && mounted) await change({'pin': pin.trim()});
  }

  Future<void> diagnostics() async {
    final Uri? instanceStatus;
    try {
      instanceStatus = widget.location.pinned
          ? widget.location.api('status')
          : null;
    } on FormatException {
      return feedback('音箱地址无效，请从右下角重新选择音箱。');
    }
    await showGlassDialog<void>(
      context: context,
      builder: (_) => _Diagnostics(
        api: api,
        instanceStatus: instanceStatus,
        export: exportLogs,
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final current = config;
    if (loadError != null) {
      return TextButton(onPressed: load, child: Text('$loadError，点击重试'));
    }
    if (current == null) {
      return const Padding(
        padding: EdgeInsets.all(24),
        child: Center(child: CircularProgressIndicator()),
      );
    }
    final receiver = current['receiver'] as Map;
    final protocols = current['protocols'] as Map;
    final name = receiver['name'] as String;
    return SettingsView(
      state: AppState(
        initialized: true,
        busy: busy,
        receiverName: name,
        settings: AppSettings(
          receiverName: name,
          advancedEffectsEnabled: widget.advancedEffects,
          miPlayEnabled: protocols['miplay']['enabled'] as bool,
          airPlayEnabled: protocols['airplay']['enabled'] as bool,
          dlnaEnabled: protocols['dlna']['enabled'] as bool,
          miPlayDeviceIdentity: MiPlayDeviceIdentity.parse(
            protocols['miplay']['device_type'],
          ),
        ),
      ),
      scrollable: false,
      showHeader: false,
      showHostControls: false,
      productVersion: version,
      initialFocusNode: widget.initialFocus,
      onClose: widget.onClose,
      onReceiverName: (name) => change({'name': name}),
      onAirPlay: (value) => change({'airplay': value}),
      onDlna: (value) => change({'dlna': value}),
      onMiPlay: (value) => change({'miplay': value}),
      onMiPlayIdentity: (identity) =>
          change({'device_type': identity.persistedValue}),
      onStartup: (_) {},
      onAutoWake: (_) {},
      onAdvancedEffects: widget.onAdvancedEffects,
      onOpenUpdate: widget.onOpenUpdate,
      extraActions: [
        (icon: Icons.network_check_rounded, label: '网络接口', onTap: network),
        (
          icon: Icons.lock_outline_rounded,
          label: 'AirPlay 配对 PIN',
          onTap: pairing,
        ),
        (
          icon: Icons.monitor_heart_outlined,
          label: '日志与故障信息',
          onTap: diagnostics,
        ),
        (icon: Icons.download_outlined, label: '导出配置', onTap: backup),
        (icon: Icons.upload_file_outlined, label: '导入配置', onTap: restore),
      ],
      onExportLogs: exportLogs,
    );
  }
}

Future<String?> _editValue(
  BuildContext context,
  String title,
  String initial, {
  required String hint,
  bool obscureText = false,
}) {
  var value = initial;
  return showGlassDialog<String>(
    context: context,
    builder: (dialog) => Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Text(title),
        TextFormField(
          autofocus: true,
          initialValue: initial,
          obscureText: obscureText,
          onChanged: (text) => value = text,
          onFieldSubmitted: (text) => Navigator.pop(dialog, text),
          decoration: InputDecoration(hintText: hint),
        ),
        Wrap(
          children: [
            TextButton(
              onPressed: () => Navigator.pop(dialog),
              child: const Text('取消'),
            ),
            TextButton(
              onPressed: () => Navigator.pop(dialog, value),
              child: const Text('保存'),
            ),
          ],
        ),
      ],
    ),
  );
}

class _Diagnostics extends StatefulWidget {
  const _Diagnostics({
    required this.api,
    required this.instanceStatus,
    required this.export,
  });

  final Uri Function(String) api;
  final Uri? instanceStatus;
  final Future<void> Function() export;

  @override
  State<_Diagnostics> createState() => _DiagnosticsState();
}

class _DiagnosticsState extends State<_Diagnostics> {
  List<String> lines = const [];
  List<String> faults = const [];
  String engine = '';
  String soundDevices = '';
  String logQuery = '';
  String level = 'info';
  String? error;
  bool loading = true;

  @override
  void initState() {
    super.initState();
    refresh();
  }

  Future<void> refresh({String? newLevel}) async {
    setState(() {
      loading = true;
      error = null;
    });
    try {
      if (newLevel != null) {
        await fnosRequest(
          widget.api('logs/level'),
          method: 'POST',
          body: {'level': newLevel},
        );
      }
      final root = widget.api('status');
      final instance = widget.instanceStatus;
      final results = await Future.wait([
        fnosRequest(root),
        fnosRequest(widget.api('logs?lines=200')),
        fnosRequest(widget.api('diagnostics')),
        if (instance != null)
          fnosRequest(instance).catchError((Object _) => <String, dynamic>{}),
      ]);
      if (!mounted) return;
      setState(() {
        level = results.first['logging']['level'] as String? ?? 'info';
        lines = (results[1]['lines'] as List)
            .map((line) => line.toString())
            .toList();
        soundDevices = [results[2]['alsa_cards'], results[2]['alsa_pcm']]
            .whereType<String>()
            .where((part) => part.trim().isNotEmpty)
            .join('\n');
        engine = [
          results[2]['backend'],
          ...(results[2]['interfaces'] as List),
        ].join(' · ');
        faults = {
          for (final status in [
            results.first,
            if (results.length == 4) results.last,
          ])
            for (final fault in (status['errors'] as List? ?? []))
              fault.toString(),
        }.toList().reversed.toList();
      });
    } catch (failure) {
      if (mounted) setState(() => error = '读取诊断失败：$failure');
    } finally {
      if (mounted) setState(() => loading = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final visibleLogs = lines
        .where((line) => line.toLowerCase().contains(logQuery))
        .join('\n');
    return SizedBox(
      width: 520,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('日志与故障信息', style: Theme.of(context).textTheme.titleLarge),
          if (engine.isNotEmpty) Text(engine),
          if (loading) const LinearProgressIndicator(),
          if (error != null) Text(error!),
          TextField(
            decoration: const InputDecoration(labelText: '搜索日志'),
            onChanged: (value) =>
                setState(() => logQuery = value.trim().toLowerCase()),
          ),
          Wrap(
            children: [
              for (final item in ['error', 'warn', 'info', 'debug', 'trace'])
                ChoiceChip(
                  label: Text(item),
                  selected: item == level,
                  onSelected: loading ? null : (_) => refresh(newLevel: item),
                ),
            ],
          ),
          Flexible(
            child: SingleChildScrollView(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  if (soundDevices.isNotEmpty)
                    ExpansionTile(
                      title: const Text('ALSA 声卡与输出'),
                      children: [SelectableText(soundDevices)],
                    ),
                  const Text('故障信息'),
                  SelectableText(faults.isEmpty ? '暂无记录' : faults.join('\n\n')),
                  const SizedBox(height: 16),
                  const Text('运行日志'),
                  SelectableText(visibleLogs.isEmpty ? '暂无匹配日志' : visibleLogs),
                ],
              ),
            ),
          ),
          Wrap(
            children: [
              TextButton(
                onPressed: loading ? null : () => refresh(),
                child: const Text('刷新'),
              ),
              TextButton(onPressed: widget.export, child: const Text('导出日志')),
              TextButton(
                onPressed: () => Navigator.pop(context),
                child: const Text('关闭'),
              ),
            ],
          ),
        ],
      ),
    );
  }
}
