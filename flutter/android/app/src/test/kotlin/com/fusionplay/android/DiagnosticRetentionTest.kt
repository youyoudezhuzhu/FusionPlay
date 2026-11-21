package com.fusionplay.android

import com.airplayreceiver.desktop.backend.FusionPlayDiagnosticLogger
import java.io.File
import java.time.Clock
import java.time.Instant
import java.time.ZoneOffset
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class DiagnosticRetentionTest {
    @get:Rule val folder = TemporaryFolder()

    @Test fun expiredAndAbandonedExportsAreReclaimedWithoutAnotherExport() {
        val dir = folder.newFolder()
        val now = System.currentTimeMillis()
        val old = File(dir, "FusionPlay-logs-20260101-000000.zip").apply {
            writeText("old"); setLastModified(now - 4L * 86400000)
        }
        val staging = File(dir, "staging-20260101-000000-1").apply { mkdir() }
        val other = File(dir, "personal.zip").apply { writeText("keep") }
        repeat(5) { File(dir, "FusionPlay-logs-20261121-00000$it.zip").writeText("new") }
        FusionPlayLogExporter.cleanDirectory(dir, now = now)
        assertFalse(old.exists())
        assertFalse(staging.exists())
        assertTrue(other.exists())
        assertEquals(4, dir.listFiles()!!.size)
    }

    @Test fun startupExpiresInactiveSegmentsAndKeepsRecentErrorSnapshot() {
        val dir = folder.newFolder()
        val snapshot = folder.newFolder().toPath()
        val now = Instant.parse("2026-11-21T00:00:00Z")
        File(dir, "fusionplay.jsonl.1").apply {
            writeText("old"); setLastModified(now.minusSeconds(8L * 86400).toEpochMilli())
        }
        File(dir, "errors.jsonl").apply { writeText("error"); setLastModified(now.toEpochMilli()) }
        val logger = FusionPlayDiagnosticLogger(dir.toPath(), Clock.fixed(now, ZoneOffset.UTC))
        logger.copySnapshotTo(snapshot)
        assertFalse(File(dir, "fusionplay.jsonl.1").exists())
        assertEquals("error", snapshot.resolve("errors.jsonl").toFile().readText())
    }
}
