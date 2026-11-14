package com.fusionplay.android.update

/**
 * Android 在线更新的固定配置（单仓库、单通道）。
 *
 * 清单发布在 FusionPlay 仓库的 GitHub Pages：`docs/update/` 经 Actions artifact
 * 部署到 `https://youyoudezhuzhu.github.io/FusionPlay/`；同一份清单也会由 CI
 * 提交到 main 的 `updates/android/stable.json` 作为 raw 兜底。
 */
object UpdateConfig {
    const val PAGES_MANIFEST_URL =
        "https://youyoudezhuzhu.github.io/FusionPlay/update/android/stable.json"

    const val RAW_MANIFEST_URL =
        "https://raw.githubusercontent.com/youyoudezhuzhu/FusionPlay/main/updates/android/stable.json"

    const val RELEASE_PAGE = "https://github.com/youyoudezhuzhu/FusionPlay/releases"

    /** 官方 release keystore 的证书 SHA-256（小写 hex），用于 APK 安装前校验。 */
    const val EXPECTED_SIGNER_SHA256 =
        "4836fb96c2ef76fd81ac385455f1b0154af77fadae1b45bbb752b192499dc12f"

    const val CHANNEL = "stable"

    const val USER_AGENT = "FusionPlay-Update/1 (Android)"

    /** 非强制检查的本地缓存时间。 */
    const val CHECK_CACHE_MS = 5 * 60 * 1000L

    const val MANIFEST_TIMEOUT_MS = 5_000

    /** 清单候选并行竞速的整体上限。 */
    const val MANIFEST_TOTAL_TIMEOUT_MS = 8_000L

    /** 单个清单最大体积（正常约 3KB）。 */
    const val MAX_MANIFEST_BYTES = 128 * 1024

    const val DOWNLOAD_TIMEOUT_MS = 180_000

    /** APK 体积上限，防止恶意清单撑爆存储。 */
    const val MAX_APK_BYTES = 256L * 1024 * 1024

    /** 下载重定向手动跟随，每跳都必须是 HTTPS。 */
    const val MAX_REDIRECTS = 5

    /** MethodChannel 名称（与 Flutter 侧 flutter/lib/backend/update_service.dart 对应）。 */
    const val METHOD_CHANNEL = "com.fusionplay.android/update"
}
