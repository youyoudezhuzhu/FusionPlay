package com.fusionplay.android.update

import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import java.io.File
import java.security.MessageDigest

/** APK 完整性校验：SHA-256、包名/版本号、签名证书。 */
object UpdateVerifier {
    fun sha256(file: File): String {
        val digest = MessageDigest.getInstance("SHA-256")
        file.inputStream().use { input ->
            val buffer = ByteArray(64 * 1024)
            while (true) {
                val read = input.read(buffer)
                if (read < 0) break
                digest.update(buffer, 0, read)
            }
        }
        return digest.digest().joinToString("") { byte -> "%02x".format(byte) }
    }

    /**
     * 校验 APK 与清单一致：包名、versionCode、签名证书 SHA-256。
     * 返回错误信息；null 表示通过。
     */
    fun verifyApk(context: Context, file: File, manifest: UpdateManifest): String? {
        @Suppress("DEPRECATION")
        val info = context.packageManager.getPackageArchiveInfo(
            file.absolutePath,
            PackageManager.GET_SIGNING_CERTIFICATES,
        ) ?: return "APK 无法解析"

        if (info.packageName != context.packageName) {
            return "APK 包名不匹配"
        }
        val archiveVersionCode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            info.longVersionCode
        } else {
            @Suppress("DEPRECATION")
            info.versionCode.toLong()
        }
        if (archiveVersionCode != manifest.versionCode) {
            return "APK 版本号与更新清单不一致"
        }

        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.P) {
            // getPackageArchiveInfo 在 API 28 以下拿不到可靠签名信息，跳过证书校验，
            // 但仍保留 SHA-256 校验作为完整性保证（1.2.7+ 需要 API 29+，实际不会走到这里）。
            return null
        }
        val signers = info.signingInfo?.apkContentsSigners
        if (signers.isNullOrEmpty()) return "APK 未签名"

        val signerSha = MessageDigest.getInstance("SHA-256")
            .digest(signers[0].toByteArray())
            .joinToString("") { byte -> "%02x".format(byte) }
        val expected = UpdateManifestParser.normalizeSha(
            manifest.signerSha256.ifBlank { UpdateConfig.EXPECTED_SIGNER_SHA256 },
        )
        if (signerSha != expected) return "APK 签名证书与预期不一致"
        return null
    }
}
