package com.fusionplay.android.update

import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.IOException

class UpdateDownloaderTest {
    @get:Rule val temp = TemporaryFolder()

    @Test
    fun reopensVerifiedFileWithoutNetworkAndRejectsDamagedCopies() {
        val source = temp.newFile().apply { writeText("test archive") }
        val manifest = UpdateManifest(apkUrl = "https://example.test/update.apk", sha256 = UpdateVerifier.sha256(source))
        val directory = temp.newFolder()
        var transfers = 0
        fun downloader() = UpdateDownloader(directory, { _, _ -> null }) { _, file, _, progress ->
            transfers++
            source.copyTo(file, overwrite = true)
            progress(file.length(), file.length())
        }
        val first = downloader().download(manifest) { _, _ -> }
        assertEquals(first, downloader().verifiedFile(manifest))
        assertEquals(first, downloader().download(manifest) { _, _ -> fail("unexpected transfer") })
        assertEquals(1, transfers)
        first.writeText("damaged")
        assertNull(downloader().verifiedFile(manifest))
        downloader().download(manifest) { _, _ -> }
        assertEquals(2, transfers)
    }

    @Test
    fun signatureFailureDoesNotExposeAPartialFile() {
        val source = temp.newFile().apply { writeText("wrong signer") }
        val manifest = UpdateManifest(apkUrl = "https://example.test/update.apk", sha256 = UpdateVerifier.sha256(source))
        val directory = temp.newFolder()
        val downloader = UpdateDownloader(directory, { _, _ -> "bad signature" }) { _, file, _, _ ->
            source.copyTo(file, overwrite = true)
        }
        assertThrows(IOException::class.java) { downloader.download(manifest) { _, _ -> } }
        assertNull(downloader.verifiedFile(manifest))
        assertTrue(directory.listFiles()!!.isEmpty())
    }
}
