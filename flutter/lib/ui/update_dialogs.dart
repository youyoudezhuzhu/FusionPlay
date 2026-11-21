import 'dart:async';

import 'package:flutter/material.dart';

import '../backend/update_service.dart';
import '../theme/fusion_theme.dart';
import 'liquid_glass.dart';

/// Shared in-app update flow (progress → result → install) used by both the
/// settings page row and the three-dot action menu.
Future<void> runUpdateFlow({
  required BuildContext context,
  required Future<UpdateCheckResult> Function() onCheckUpdate,
  DownloadUpdate? onDownloadUpdate,
  InstallUpdate? onInstallUpdate,
  SaveUpdate? saveUpdate,
  ValueChanged<bool>? onCheckingChanged,
  ValueChanged<bool>? onDownloadingChanged,
}) async {
  assert(
    saveUpdate != null || (onDownloadUpdate != null && onInstallUpdate != null),
  );
  onCheckingChanged?.call(true);
  UpdateCheckResult? result;
  try {
    result = await showBusyDialog<UpdateCheckResult>(
      context: context,
      message: '正在检查更新…',
      task: onCheckUpdate,
    );
  } finally {
    onCheckingChanged?.call(false);
  }
  if (!context.mounted) return;
  if (result == null) {
    await showUpdateMessage(context, '检查更新失败', '请稍后重试');
    return;
  }
  switch (result.status) {
    case UpdateCheckStatus.available:
      await showAvailableUpdateDialog(
        context,
        result,
        download: onDownloadUpdate,
        install: onInstallUpdate,
        saveUpdate: saveUpdate,
        onDownloadingChanged: onDownloadingChanged,
      );
    case UpdateCheckStatus.upToDate:
      await showUpdateMessage(context, '已是最新版本');
    case UpdateCheckStatus.unavailable:
      await showUpdateMessage(context, '当前无法检查更新', result.message);
    case UpdateCheckStatus.error:
      await showUpdateMessage(context, '检查更新失败', result.message ?? '请稍后重试');
  }
}

/// Shows a non-dismissible progress dialog while [task] runs and returns the
/// task result (or null when it failed).
Future<T?> showBusyDialog<T>({
  required BuildContext context,
  required String message,
  required Future<T> Function() task,
}) => showGlassDialog<T>(
  context: context,
  dialogKey: const ValueKey('update-busy-dialog'),
  barrierDismissible: false,
  builder: (_) => UpdateProgressDialog<T>(message: message, task: task),
);

Future<void> showAvailableUpdateDialog(
  BuildContext context,
  UpdateCheckResult result, {
  DownloadUpdate? download,
  InstallUpdate? install,
  SaveUpdate? saveUpdate,
  ValueChanged<bool>? onDownloadingChanged,
}) => showGlassDialog<void>(
  context: context,
  dialogKey: const ValueKey('update-dialog'),
  barrierDismissible: false,
  builder: (_) => _AvailableUpdate(
    result: result,
    download: download,
    install: install,
    saveUpdate: saveUpdate,
    onDownloadingChanged: onDownloadingChanged,
  ),
);

class _AvailableUpdate extends StatefulWidget {
  const _AvailableUpdate({
    required this.result,
    required this.download,
    required this.install,
    this.saveUpdate,
    this.onDownloadingChanged,
  });
  final UpdateCheckResult result;
  final DownloadUpdate? download;
  final InstallUpdate? install;
  final SaveUpdate? saveUpdate;
  final ValueChanged<bool>? onDownloadingChanged;

  @override
  State<_AvailableUpdate> createState() => _AvailableUpdateState();
}

class _AvailableUpdateState extends State<_AvailableUpdate> {
  late bool ready = widget.saveUpdate == null && widget.result.downloaded;
  bool busy = false;
  int bytes = 0;
  int total = 0;
  String? message;
  String? savedPath;

  Future<void> _act() async {
    if (busy) return;
    setState(() {
      busy = true;
      message = null;
    });
    final downloading = !ready;
    final onDownloadingChanged = widget.onDownloadingChanged;
    try {
      if (downloading) {
        bytes = 0;
        total = 0;
        onDownloadingChanged?.call(true);
        void progress(int received, int expected) {
          if (!mounted) return;
          setState(() {
            bytes = received;
            total = expected;
          });
        }

        final save = widget.saveUpdate;
        if (save != null) {
          final path = await save(widget.result.releaseKey, progress);
          if (path == null) return;
          savedPath = path;
        } else {
          await widget.download!(widget.result.releaseKey, progress);
        }
        if (mounted) setState(() => ready = true);
      } else {
        final result = await widget.install!(widget.result.releaseKey);
        if (!mounted) return;
        setState(() {
          if (result.status == UpdateInstallStatus.missing) ready = false;
          message = switch (result.status) {
            UpdateInstallStatus.started => '安装器已打开',
            UpdateInstallStatus.permissionRequired =>
              '请在系统设置中允许安装未知应用，返回后点击「安装」。',
            _ => result.message ?? '启动安装器失败',
          };
        });
      }
    } catch (error) {
      if (mounted) {
        setState(
          () =>
              message = error is StateError ? error.message : error.toString(),
        );
      }
    } finally {
      if (downloading) onDownloadingChanged?.call(false);
      if (mounted) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final result = widget.result;
    final downloading = busy && !ready;
    final expected = total > 0 ? total : result.apkSize ?? 0;
    final fraction = expected > 0 ? (bytes / expected).clamp(0.0, 1.0) : null;
    return PopScope(
      canPop: !busy,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          _GlassDialogTitle('发现新版本 ${result.versionName ?? ''}'.trim()),
          const SizedBox(height: 10),
          Flexible(
            child: SingleChildScrollView(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  if (savedPath != null)
                    SelectableText('已保存到 $savedPath\n请从此位置手动运行安装程序。'),
                  if (result.publishedAt?.isNotEmpty == true)
                    _updateDetail('发布时间：${result.publishedAt}'),
                  if ((result.apkSize ?? 0) > 0)
                    _updateDetail('安装包大小：${_formatBytes(result.apkSize!)}'),
                  const SizedBox(height: 6),
                  Text(
                    result.notes?.trim().isNotEmpty == true
                        ? result.notes!.trim()
                        : '本次更新未提供更新说明。',
                  ),
                ],
              ),
            ),
          ),
          const SizedBox(height: 12),
          if (downloading || ready) ...[
            LinearProgressIndicator(
              value: ready ? 1.0 : fraction,
              semanticsLabel: '下载进度',
            ),
            const SizedBox(height: 4),
            _updateDetail(
              ready
                  ? '下载完成，安装包已校验'
                  : fraction == 1.0
                  ? '正在校验安装包…'
                  : fraction == null
                  ? '已下载 ${_formatBytes(bytes)}'
                  : '已下载 ${(fraction * 100).floor()}%',
            ),
          ],
          if (message != null) Text(message!),
          const SizedBox(height: 8),
          _GlassDialogActions(
            children: [
              _GlassDialogButton(
                label: ready ? '关闭' : '取消',
                onPressed: busy ? null : () => Navigator.of(context).pop(),
              ),
              if (widget.saveUpdate == null || !ready)
                _GlassDialogButton(
                  label: ready
                      ? '安装'
                      : downloading
                      ? '正在下载…'
                      : widget.saveUpdate == null
                      ? '下载更新'
                      : '选择保存目录',
                  primary: true,
                  onPressed: busy ? null : _act,
                ),
            ],
          ),
        ],
      ),
    );
  }
}

Widget _updateDetail(String text) => Padding(
  padding: const EdgeInsets.only(bottom: 4),
  child: Text(
    text,
    style: const TextStyle(
      fontSize: 13,
      color: FusionGlassColors.supportingText,
    ),
  ),
);

Future<void> showUpdateMessage(
  BuildContext context,
  String title, [
  String? message,
]) => showGlassDialog<void>(
  context: context,
  dialogKey: const ValueKey('update-message-dialog'),
  builder: (dialogContext) => Column(
    mainAxisSize: MainAxisSize.min,
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      _GlassDialogTitle(title),
      if (message != null && message.trim().isNotEmpty) ...[
        const SizedBox(height: 10),
        Text(
          message,
          style: const TextStyle(
            fontSize: 14,
            height: 1.5,
            color: Color(0xB8FFFFFF),
          ),
        ),
      ],
      const SizedBox(height: 20),
      _GlassDialogActions(
        children: [
          _GlassDialogButton(
            label: '好',
            primary: true,
            onPressed: () => Navigator.of(dialogContext).pop(),
          ),
        ],
      ),
    ],
  ),
);

class _GlassDialogTitle extends StatelessWidget {
  const _GlassDialogTitle(this.text);

  final String text;

  @override
  Widget build(BuildContext context) => Text(
    text,
    style: const TextStyle(
      fontSize: 18,
      color: Color(0xEBFFFFFF),
      fontWeight: FusionTypography.semiBoldFontWeight,
      fontVariations: FusionTypography.semiBold,
    ),
  );
}

class _GlassDialogActions extends StatelessWidget {
  const _GlassDialogActions({required this.children});

  final List<Widget> children;

  @override
  Widget build(BuildContext context) => Row(
    mainAxisAlignment: MainAxisAlignment.end,
    children: [
      for (var index = 0; index < children.length; index++) ...[
        if (index > 0) const SizedBox(width: 10),
        children[index],
      ],
    ],
  );
}

class _GlassDialogButton extends StatelessWidget {
  const _GlassDialogButton({
    required this.label,
    required this.onPressed,
    this.primary = false,
  });

  final String label;
  final VoidCallback? onPressed;
  final bool primary;

  @override
  Widget build(BuildContext context) => FilledButton(
    style: FilledButton.styleFrom(
      backgroundColor: primary
          ? const Color(0xE6FFFFFF)
          : const Color(0x14FFFFFF),
      foregroundColor: primary
          ? const Color(0xFF16181B)
          : const Color(0xEBFFFFFF),
      padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 12),
      shape: const StadiumBorder(),
      side: primary ? null : const BorderSide(color: Color(0x1FFFFFFF)),
    ),
    onPressed: onPressed,
    child: Text(
      label,
      style: const TextStyle(
        fontSize: 14,
        fontWeight: FusionTypography.mediumFontWeight,
        fontVariations: FusionTypography.medium,
      ),
    ),
  );
}

class UpdateProgressDialog<T> extends StatefulWidget {
  const UpdateProgressDialog({
    super.key,
    required this.message,
    required this.task,
  });

  final String message;
  final Future<T> Function() task;

  @override
  State<UpdateProgressDialog<T>> createState() =>
      _UpdateProgressDialogState<T>();
}

class _UpdateProgressDialogState<T> extends State<UpdateProgressDialog<T>> {
  @override
  void initState() {
    super.initState();
    unawaited(_run());
  }

  Future<void> _run() async {
    T? result;
    var failed = false;
    try {
      result = await widget.task();
    } catch (_) {
      failed = true;
    }
    if (!mounted) return;
    Navigator.of(context).pop(failed ? null : result);
  }

  @override
  Widget build(BuildContext context) => PopScope(
    canPop: false,
    child: Row(
      children: [
        const SizedBox.square(
          dimension: 22,
          child: CircularProgressIndicator(
            strokeWidth: 2.5,
            color: Color(0xEBFFFFFF),
          ),
        ),
        const SizedBox(width: 16),
        Flexible(
          child: Text(
            widget.message,
            style: const TextStyle(fontSize: 15, color: Color(0xEBFFFFFF)),
          ),
        ),
      ],
    ),
  );
}

String _formatBytes(int bytes) {
  if (bytes >= 1024 * 1024) {
    return '${(bytes / (1024 * 1024)).toStringAsFixed(1)} MB';
  }
  if (bytes >= 1024) {
    return '${(bytes / 1024).toStringAsFixed(0)} KB';
  }
  return '$bytes B';
}
