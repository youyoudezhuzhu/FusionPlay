package com.fusionplay.android.update

import android.content.Context
import android.os.Handler
import android.os.Looper
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

/**
 * Android 在线更新：检查清单、下载/校验 APK、拉起系统安装器。
 *
 * Flutter 侧通过 `com.fusionplay.android/update` 通道调用：
 * - `check` → { status: available|up_to_date|unavailable|error, ... }
 * - `download` reports byte progress and returns ready after verification.
 * - `install` opens the installer for a verified local package.
 * - legacy `downloadAndInstall` → { status: started|permission_required|error, message? }
 *
 * 网络与文件操作都在 IO 线程执行，结果切回主线程再回给 Flutter。
 */
class UpdateManager(private val context: Context) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val mainHandler = Handler(Looper.getMainLooper())
    private val repository = UpdateRepository(context)
    private val downloader = UpdateDownloader(context)
    private val installer = UpdateInstaller(context)
    private companion object {
        // Activity recreation may attach another manager while IO is winding down.
        val busy = AtomicBoolean(false)
    }

    @Volatile private var selected: UpdateManifest? = null
    private var channel: MethodChannel? = null

    fun attach(messenger: BinaryMessenger) {
        val methodChannel = MethodChannel(messenger, UpdateConfig.METHOD_CHANNEL)
        methodChannel.setMethodCallHandler(::onMethodCall)
        channel = methodChannel
    }

    fun close() {
        channel?.setMethodCallHandler(null)
        channel = null
    }

    private fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            "check" -> scope.launch {
                val outcome = runCatching { repository.check(force = true) }
                    .getOrElse { UpdateCheck.Failure(it.message ?: "检查更新失败") }
                val available = (outcome as? UpdateCheck.Available)?.manifest
                selected = available
                val downloaded = available != null && runCatching {
                    downloader.verifiedFile(available) != null
                }.getOrDefault(false)
                respond(result, checkMap(outcome) + ("downloaded" to downloaded))
            }
            "download", "install", "downloadAndInstall" -> {
                if (!busy.compareAndSet(false, true)) {
                    result.success(
                        mapOf("status" to "error", "message" to "已有更新任务正在进行中"),
                    )
                    return
                }
                scope.launch {
                    val payload = try {
                        if (call.method == "downloadAndInstall") downloadAndInstall() else updateAction(call)
                    } catch (error: Exception) {
                        mapOf("status" to "error", "message" to (error.message ?: "更新失败"))
                    } finally {
                        busy.set(false)
                    }
                    respond(result, payload)
                }
            }
            else -> result.notImplemented()
        }
    }

    private fun updateAction(call: MethodCall): Map<String, Any?> {
        val manifest = selected
        require(manifest != null && manifest.sha256 == call.argument<String>("releaseKey")) {
            "更新版本已变化，请重新检查更新"
        }
        if (call.method == "install") {
            val apk = downloader.verifiedFile(manifest)
                ?: return mapOf("status" to "missing", "message" to "安装包已失效，请重新下载")
            return install(apk)
        }
        downloader.download(manifest) { bytes, total ->
            val event = mapOf("bytes" to bytes, "total" to total, "releaseKey" to manifest.sha256)
            mainHandler.post { channel?.invokeMethod("progress", event) }
        }
        return mapOf("status" to "ready")
    }

    private fun downloadAndInstall(): Map<String, Any?> {
        val check = repository.check(force = true)
        val manifest = when (check) {
            is UpdateCheck.Available -> check.manifest
            is UpdateCheck.UpToDate -> return mapOf(
                "status" to "error",
                "message" to "当前已是最新版本（${check.manifest.versionName}）",
            )
            is UpdateCheck.Unavailable -> return mapOf(
                "status" to "error",
                "message" to "更新源暂不可用：${check.message}",
            )
            is UpdateCheck.Failure -> return mapOf(
                "status" to "error",
                "message" to "检查更新失败：${check.message}",
            )
        }
        return install(downloader.download(manifest) { _, _ -> })
    }

    private fun install(apk: java.io.File): Map<String, Any?> = when (val install = installer.install(apk)) {
        is InstallResult.Started -> mapOf("status" to "started")
        is InstallResult.PermissionRequired -> mapOf(
            "status" to "permission_required",
            "message" to install.message,
        )
        is InstallResult.Failure -> mapOf(
            "status" to "error",
            "message" to install.message,
        )
    }

    private fun checkMap(outcome: UpdateCheck): Map<String, Any?> = when (outcome) {
        is UpdateCheck.Available -> mapOf(
            "status" to "available",
            "releaseKey" to outcome.manifest.sha256,
            "versionName" to outcome.manifest.versionName,
            "versionCode" to outcome.manifest.versionCode,
            "notes" to outcome.manifest.notes,
            "publishedAt" to outcome.manifest.publishedAt,
            "apkSize" to outcome.manifest.apkSize,
        )
        is UpdateCheck.UpToDate -> mapOf(
            "status" to "up_to_date",
            "versionName" to outcome.manifest.versionName,
            "versionCode" to outcome.manifest.versionCode,
        )
        is UpdateCheck.Unavailable -> mapOf(
            "status" to "unavailable",
            "message" to outcome.message,
        )
        is UpdateCheck.Failure -> mapOf(
            "status" to "error",
            "message" to outcome.message,
        )
    }

    private fun respond(result: MethodChannel.Result, payload: Map<String, Any?>) {
        mainHandler.post { runCatching { result.success(payload) } }
    }
}
