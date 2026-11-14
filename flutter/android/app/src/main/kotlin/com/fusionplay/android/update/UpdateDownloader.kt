package com.fusionplay.android.update

import android.content.Context
import java.io.File
import java.io.IOException

/** 下载并校验更新 APK；成功时返回缓存目录中的 APK 文件。 */
class UpdateDownloader(private val context: Context) {
    fun download(manifest: UpdateManifest): File {
        val directory = File(context.cacheDir, "updates").apply { mkdirs() }
        val expectedSha = UpdateManifestParser.normalizeSha(manifest.sha256)
        val target = File(directory, "FusionPlay-Android-${manifest.versionName}.apk")
        var lastError: String? = null

        for (url in UpdateManifestParser.downloadCandidates(manifest)) {
            val partial = File(directory, "${target.name}.part")
            try {
                if (partial.exists()) partial.delete()
                downloadTo(url, partial)
                if (partial.length() <= 0L) throw IOException("下载内容为空")
                if (partial.length() > UpdateConfig.MAX_APK_BYTES) {
                    throw IOException("APK 体积超出上限")
                }
                val actualSha = UpdateVerifier.sha256(partial)
                if (actualSha != expectedSha) {
                    throw IOException("APK 校验失败（SHA-256 不匹配）")
                }
                if (target.exists() && !target.delete()) {
                    throw IOException("无法替换旧的更新包")
                }
                if (!partial.renameTo(target)) {
                    partial.copyTo(target, overwrite = true)
                    partial.delete()
                }
                val verifyError = UpdateVerifier.verifyApk(context, target, manifest)
                if (verifyError != null) {
                    target.delete()
                    throw IOException(verifyError)
                }
                directory.listFiles()
                    ?.filter { it.isFile && it.name.endsWith(".apk") && it.name != target.name }
                    ?.forEach { it.delete() }
                return target
            } catch (error: Exception) {
                lastError = error.message ?: "下载失败"
                runCatching { partial.delete() }
            }
        }
        throw IOException(lastError ?: "没有可用的下载地址")
    }

    private fun downloadTo(url: String, destination: File) {
        UpdateHttp.requireHttps(url)
        val connection = UpdateHttp.open(
            url,
            UpdateConfig.MANIFEST_TIMEOUT_MS,
            UpdateConfig.DOWNLOAD_TIMEOUT_MS,
            "application/octet-stream, */*",
        )
        try {
            connection.inputStream.use { input ->
                destination.outputStream().use { output ->
                    val buffer = ByteArray(64 * 1024)
                    var total = 0L
                    while (true) {
                        val read = input.read(buffer)
                        if (read < 0) break
                        total += read
                        if (total > UpdateConfig.MAX_APK_BYTES) {
                            throw IOException("APK 体积超出上限")
                        }
                        output.write(buffer, 0, read)
                    }
                    output.flush()
                }
            }
        } finally {
            connection.disconnect()
        }
    }
}
