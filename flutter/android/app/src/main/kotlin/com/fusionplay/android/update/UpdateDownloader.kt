package com.fusionplay.android.update

import android.content.Context
import java.io.File
import java.io.IOException

/** 下载并校验更新 APK；成功时返回缓存目录中的 APK 文件。 */
internal class UpdateDownloader(
    private val directory: File,
    private val verifyArchive: (File, UpdateManifest) -> String?,
    private val transfer: (String, File, Long, (Long, Long) -> Unit) -> Unit = ::downloadTo,
) {
    constructor(context: Context) : this(
        File(context.cacheDir, "updates"),
        { file, manifest -> UpdateVerifier.verifyApk(context, file, manifest) },
    )

    private fun target(manifest: UpdateManifest) =
        File(directory, "FusionPlay-${UpdateManifestParser.normalizeSha(manifest.sha256)}.apk")

    fun verifiedFile(manifest: UpdateManifest): File? {
        val file = target(manifest)
        if (!file.isFile || file.length() !in 1..UpdateConfig.MAX_APK_BYTES) return null
        return file.takeIf {
            UpdateVerifier.sha256(it) == UpdateManifestParser.normalizeSha(manifest.sha256) &&
                verifyArchive(it, manifest) == null
        }
    }

    fun download(manifest: UpdateManifest, progress: (Long, Long) -> Unit): File {
        verifiedFile(manifest)?.let { return it }
        directory.mkdirs()
        val expectedSha = UpdateManifestParser.normalizeSha(manifest.sha256)
        val target = target(manifest)
        var lastError: String? = null

        for (url in UpdateManifestParser.downloadCandidates(manifest)) {
            val partial = File(directory, "${target.nameWithoutExtension}.partial.apk")
            try {
                if (partial.exists()) partial.delete()
                transfer(url, partial, manifest.apkSize, progress)
                if (partial.length() <= 0L) throw IOException("下载内容为空")
                if (partial.length() > UpdateConfig.MAX_APK_BYTES) {
                    throw IOException("APK 体积超出上限")
                }
                val actualSha = UpdateVerifier.sha256(partial)
                if (actualSha != expectedSha) {
                    throw IOException("APK 校验失败（SHA-256 不匹配）")
                }
                verifyArchive(partial, manifest)?.let { throw IOException(it) }
                // Both files live in the same private directory; expose only a complete, verified APK.
                if (!partial.renameTo(target)) throw IOException("无法保存已校验的更新包")
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
}

private fun downloadTo(url: String, destination: File, expectedSize: Long, progress: (Long, Long) -> Unit) {
    UpdateHttp.requireHttps(url)
    val connection = UpdateHttp.open(
        url,
        UpdateConfig.MANIFEST_TIMEOUT_MS,
        UpdateConfig.DOWNLOAD_TIMEOUT_MS,
        "application/octet-stream, */*",
    )
    try {
        val length = expectedSize.takeIf { it > 0 } ?: connection.contentLength.toLong()
        progress(0, length)
        connection.inputStream.use { input ->
            destination.outputStream().use { output ->
                val buffer = ByteArray(64 * 1024)
                var total = 0L
                var lastReported = 0L
                while (true) {
                    val read = input.read(buffer)
                    if (read < 0) break
                    total += read
                    if (total > UpdateConfig.MAX_APK_BYTES) {
                        throw IOException("APK 体积超出上限")
                    }
                    output.write(buffer, 0, read)
                    if (total - lastReported >= 128 * 1024) {
                        progress(total, length)
                        lastReported = total
                    }
                }
                output.flush()
                progress(total, length)
            }
        }
    } finally {
        connection.disconnect()
    }
}
