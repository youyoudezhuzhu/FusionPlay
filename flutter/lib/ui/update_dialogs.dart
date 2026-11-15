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
  required Future<UpdateInstallResult> Function() onInstallUpdate,
  ValueChanged<bool>? onCheckingChanged,
  ValueChanged<bool>? onDownloadingChanged,
}) async {
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
      final install = await showAvailableUpdateDialog(context, result);
      if (!context.mounted || !install) return;
      await _downloadAndInstall(
        context,
        onInstallUpdate,
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

Future<void> _downloadAndInstall(
  BuildContext context,
  Future<UpdateInstallResult> Function() onInstall, {
  ValueChanged<bool>? onDownloadingChanged,
}) async {
  onDownloadingChanged?.call(true);
  UpdateInstallResult? result;
  try {
    result = await showBusyDialog<UpdateInstallResult>(
      context: context,
      message: '正在下载更新…',
      task: onInstall,
    );
  } finally {
    onDownloadingChanged?.call(false);
  }
  if (!context.mounted) return;
  if (result == null) {
    await showUpdateMessage(context, '下载更新失败', '请稍后重试');
    return;
  }
  switch (result.status) {
    case UpdateInstallStatus.started:
      await showUpdateMessage(context, '安装器已打开', '请在系统安装界面完成更新。');
    case UpdateInstallStatus.permissionRequired:
      await showUpdateMessage(
        context,
        '需要安装权限',
        '请在系统设置中允许「FusionPlay」安装未知应用后重试。',
      );
    case UpdateInstallStatus.error:
      await showUpdateMessage(context, '下载更新失败', result.message ?? '请稍后重试');
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

Future<bool> showAvailableUpdateDialog(
  BuildContext context,
  UpdateCheckResult result,
) async {
  final version = result.versionName?.trim();
  final title = version == null || version.isEmpty ? '发现新版本' : '发现新版本 $version';
  final notes = result.notes?.trim();
  final publishedAt = result.publishedAt?.trim();
  final apkSize = result.apkSize;
  final accepted = await showGlassDialog<bool>(
    context: context,
    dialogKey: const ValueKey('update-dialog'),
    builder: (dialogContext) => Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _GlassDialogTitle(title),
        const SizedBox(height: 10),
        if (publishedAt != null && publishedAt.isNotEmpty)
          _updateDetail('发布时间：$publishedAt'),
        if (apkSize != null && apkSize > 0)
          _updateDetail('安装包大小：${_formatBytes(apkSize)}'),
        if (notes != null && notes.isNotEmpty) ...[
          if ((publishedAt != null && publishedAt.isNotEmpty) ||
              (apkSize != null && apkSize > 0))
            const SizedBox(height: 12),
          Flexible(
            child: SingleChildScrollView(
              child: Text(
                notes,
                style: const TextStyle(
                  fontSize: 14,
                  height: 1.5,
                  color: Color(0xB8FFFFFF),
                ),
              ),
            ),
          ),
        ] else
          const Text(
            '本次更新未提供更新说明。',
            style: TextStyle(
              fontSize: 14,
              height: 1.5,
              color: Color(0x8CFFFFFF),
            ),
          ),
        const SizedBox(height: 20),
        _GlassDialogActions(
          children: [
            _GlassDialogButton(
              label: '取消',
              onPressed: () => Navigator.of(dialogContext).pop(false),
            ),
            _GlassDialogButton(
              label: '下载并安装',
              primary: true,
              onPressed: () => Navigator.of(dialogContext).pop(true),
            ),
          ],
        ),
      ],
    ),
  );
  return accepted ?? false;
}

Widget _updateDetail(String text) => Padding(
  padding: const EdgeInsets.only(bottom: 4),
  child: Text(
    text,
    style: const TextStyle(fontSize: 13, color: Color(0x8CFFFFFF)),
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
  final VoidCallback onPressed;
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
