package com.fusionplay.android.update

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class UpdateManifestTest {
    private val validJson = """
        {
          "schema_version": 1,
          "channel": "stable",
          "published": true,
          "version_name": "1.3.4",
          "version_code": 10304,
          "apk_url": "https://youyoudezhuzhu.github.io/FusionPlay/update/android/dl/latest.apk?v=10304",
          "apk_url_alt": "https://github.com/youyoudezhuzhu/FusionPlay/releases/download/android-v1.3.4/FusionPlay-Android-1.3.4.apk",
          "sha256": "45866bf4f9035f8aa0426de03e429bd3473ac9e6e26e3460779d2c976764a75d",
          "signer_sha256": "4836fb96c2ef76fd81ac385455f1b0154af77fadae1b45bbb752b192499dc12f",
          "apk_size": 31398445,
          "notes": "test",
          "published_at": "2026-11-14T16:00:00Z",
          "source_commit": "abc123"
        }
    """.trimIndent()

    @Test
    fun parsesValidManifest() {
        val manifest = UpdateManifestParser.parse(validJson)
        assertNotNull(manifest)
        manifest!!
        assertEquals(1, manifest.schemaVersion)
        assertEquals("1.3.4", manifest.versionName)
        assertEquals(10304L, manifest.versionCode)
        assertEquals(31398445L, manifest.apkSize)
        assertNull(UpdateManifestParser.errorOf(manifest))
    }

    @Test
    fun rejectsInvalidManifests() {
        val wrongSchema = UpdateManifestParser.parse(validJson)!!.copy(schemaVersion = 2)
        assertEquals("更新清单版本不支持", UpdateManifestParser.errorOf(wrongSchema))

        val badSha = UpdateManifestParser.parse(validJson)!!.copy(sha256 = "abc")
        assertEquals("更新清单缺少有效的 SHA-256", UpdateManifestParser.errorOf(badSha))

        val httpsOnly = UpdateManifestParser.parse(validJson)!!.copy(
            apkUrl = "http://example.com/a.apk",
            apkUrlAlt = "",
        )
        assertEquals("下载地址必须使用 HTTPS", UpdateManifestParser.errorOf(httpsOnly))

        val badSigner = UpdateManifestParser.parse(validJson)!!.copy(signerSha256 = "xyz")
        assertEquals("更新清单的签名证书哈希无效", UpdateManifestParser.errorOf(badSigner))
    }

    @Test
    fun parsesGarbageAsNull() {
        assertNull(UpdateManifestParser.parse("not json"))
        assertNull(UpdateManifestParser.parse(""))
    }

    @Test
    fun comparesVersionCodesStrictly() {
        val manifest = UpdateManifestParser.parse(validJson)!!
        assertTrue(UpdateManifestParser.isNewer(manifest, 10303))
        assertFalse(UpdateManifestParser.isNewer(manifest, 10304))
        assertFalse(UpdateManifestParser.isNewer(manifest, 10305))
    }

    @Test
    fun normalizesShaAndBuildsDownloadCandidates() {
        assertEquals(
            "45866bf4f9035f8aa0426de03e429bd3473ac9e6e26e3460779d2c976764a75d",
            UpdateManifestParser.normalizeSha(
                "45:86:6B:F4 F9:03:5F:8A A0:42:6D:E0 3E:42:9B D3:47:3A C9:E6:E2 6E:34 60:77:9D:2C:97:67:64:A7:5D",
            ),
        )
        val manifest = UpdateManifestParser.parse(validJson)!!
        val candidates = UpdateManifestParser.downloadCandidates(manifest)
        assertEquals(2, candidates.size)
        assertTrue(candidates[0].startsWith("https://youyoudezhuzhu.github.io/"))
        assertTrue(candidates[1].startsWith("https://github.com/"))

        val pagesOnly = manifest.copy(apkUrlAlt = "http://insecure/")
        assertEquals(1, UpdateManifestParser.downloadCandidates(pagesOnly).size)
    }
}
