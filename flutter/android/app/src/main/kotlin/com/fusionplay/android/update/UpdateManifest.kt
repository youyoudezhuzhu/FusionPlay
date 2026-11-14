package com.fusionplay.android.update

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

/**
 * 在线更新清单（schema_version = 1）。
 *
 * 清单本身不签名，信任锚是 HTTPS 传输 + APK 的 SHA-256 + APK 签名证书校验。
 * 字段需与 CI 生成的 `update-manifest.json`、`updates/android/stable.json` 一致。
 */
@Serializable
data class UpdateManifest(
    @SerialName("schema_version") val schemaVersion: Int = 1,
    @SerialName("channel") val channel: String = "",
    @SerialName("published") val published: Boolean = true,
    @SerialName("version_name") val versionName: String = "",
    @SerialName("version_code") val versionCode: Long = 0,
    @SerialName("apk_url") val apkUrl: String = "",
    @SerialName("apk_url_alt") val apkUrlAlt: String = "",
    @SerialName("sha256") val sha256: String = "",
    @SerialName("signer_sha256") val signerSha256: String = "",
    @SerialName("apk_size") val apkSize: Long = 0,
    @SerialName("notes") val notes: String = "",
    @SerialName("published_at") val publishedAt: String = "",
    @SerialName("source_commit") val sourceCommit: String = "",
)

/** 清单解析与纯逻辑校验（不依赖 Android framework，便于 JVM 单测）。 */
object UpdateManifestParser {
    private val json = Json {
        ignoreUnknownKeys = true
        isLenient = true
    }

    fun parse(text: String): UpdateManifest? = runCatching {
        json.decodeFromString<UpdateManifest>(text)
    }.getOrNull()

    /** 返回不可用原因；null 表示清单可用。 */
    fun errorOf(manifest: UpdateManifest): String? = when {
        manifest.schemaVersion != 1 -> "更新清单版本不支持"
        !manifest.published -> "更新清单尚未发布"
        manifest.versionCode <= 0 -> "更新清单缺少版本号"
        manifest.versionName.isBlank() -> "更新清单缺少版本名"
        normalizeSha(manifest.sha256).length != 64 -> "更新清单缺少有效的 SHA-256"
        manifest.apkUrl.isBlank() && manifest.apkUrlAlt.isBlank() -> "更新清单缺少下载地址"
        !isHttpsUrl(manifest.apkUrl) && !isHttpsUrl(manifest.apkUrlAlt) -> "下载地址必须使用 HTTPS"
        manifest.signerSha256.isNotBlank() && normalizeSha(manifest.signerSha256).length != 64 ->
            "更新清单的签名证书哈希无效"
        else -> null
    }

    fun isHttpsUrl(url: String): Boolean =
        url.startsWith("https://") && url.length > "https://".length

    /** 去掉用户复制时可能带入的空格/连字符，统一小写。 */
    fun normalizeSha(sha: String): String = sha.filter { it.isLetterOrDigit() }.lowercase()

    /** 只有 version_code 严格大于本机才提示更新。 */
    fun isNewer(manifest: UpdateManifest, currentVersionCode: Long): Boolean =
        manifest.versionCode > currentVersionCode

    /** 下载候选：Pages 主地址优先，GitHub Release 资产兜底，只保留 HTTPS。 */
    fun downloadCandidates(manifest: UpdateManifest): List<String> = listOf(
        manifest.apkUrl.trim(),
        manifest.apkUrlAlt.trim(),
    ).filter { isHttpsUrl(it) }.distinct()
}
