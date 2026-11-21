package com.fusionplay.android

import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Process
import androidx.core.content.FileProvider
import androidx.core.content.pm.PackageInfoCompat
import com.airplayreceiver.desktop.backend.FusionPlayDiagnosticLogger
import java.io.BufferedOutputStream
import java.io.File
import java.io.FileOutputStream
import java.text.SimpleDateFormat
import java.time.Instant
import java.util.Date
import java.util.Locale
import java.util.TimeZone
import java.util.zip.ZipEntry
import java.util.zip.ZipOutputStream
import org.json.JSONArray
import org.json.JSONObject

internal object FusionPlayLogExporter {
    private const val EXPORT_DIRECTORY = "log-exports"
    private const val ARCHIVE_RETENTION_MS = 3L * 24L * 60L * 60L * 1_000L
    private const val MAXIMUM_LOGCAT_LINES = 4_000

    private const val MAX_ARCHIVE_BYTES = 48L * 1024 * 1024
    private const val MAX_ARCHIVES = 3

    @Synchronized
    fun maintain(context: Context) = cleanDirectory(File(context.cacheDir, EXPORT_DIRECTORY))

    @Synchronized
    fun createShareIntent(
        context: Context,
        logger: FusionPlayDiagnosticLogger,
    ): Intent {
        val exportRoot = File(context.cacheDir, EXPORT_DIRECTORY).apply { mkdirs() }
        cleanDirectory(exportRoot)

        val timestamp = archiveTimestamp() + "-" + java.util.UUID.randomUUID()
        val staging = File(exportRoot, "staging-$timestamp-${Process.myPid()}")
        val archive = File(exportRoot, "FusionPlay-logs-$timestamp.zip")
        check(staging.mkdirs()) { "无法创建日志导出暂存目录。" }

        try {
            writeManifest(context, File(staging, "manifest.json"))
            val snapshots = logger.copySnapshotTo(
                File(staging, "persistent").toPath(),
            )
            writeServiceLogs(snapshots.map { it.toFile() }, File(staging, "services"))
            writeLogcat(logger, File(staging, "android-logcat.txt"))
            val partial = File(staging, "archive.tmp")
            zipDirectory(staging, partial)
            check(partial.renameTo(archive)) { "无法完成日志归档。" }
            cleanDirectory(exportRoot, archive)
        } finally {
            staging.deleteRecursively()
        }

        val authority = "${context.packageName}.logprovider"
        val uri = FileProvider.getUriForFile(context, authority, archive)
        val send = Intent(Intent.ACTION_SEND).apply {
            type = "application/zip"
            putExtra(Intent.EXTRA_STREAM, uri)
            putExtra(Intent.EXTRA_SUBJECT, archive.name)
            clipData = ClipData.newRawUri("FusionPlay logs", uri)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        return Intent.createChooser(send, "导出 FusionPlay 日志").apply {
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
    }

    private fun writeManifest(context: Context, destination: File) {
        val packageInfo = context.packageManager.getPackageInfo(context.packageName, 0)
        val manifest = JSONObject()
            .put("schema_version", 2)
            .put("exported_at_utc", Instant.now().toString())
            .put("package_name", context.packageName)
            .put("version_name", packageInfo.versionName.orEmpty())
            .put("version_code", PackageInfoCompat.getLongVersionCode(packageInfo))
            .put("android_sdk", Build.VERSION.SDK_INT)
            .put("android_release", Build.VERSION.RELEASE.orEmpty())
            .put("manufacturer", Build.MANUFACTURER.orEmpty())
            .put("model", Build.MODEL.orEmpty())
            .put("supported_abis", Build.SUPPORTED_ABIS.joinToString(","))
            .put("locale", Locale.getDefault().toLanguageTag())
            .put(
                "service_logs",
                JSONArray(
                    listOf(
                        "services/miplay.jsonl",
                        "services/airplay.jsonl",
                        "services/dlna.jsonl",
                    ),
                ),
            )
        destination.writeText(manifest.toString(2), Charsets.UTF_8)
    }

    private fun writeServiceLogs(
        snapshots: List<File>,
        destination: File,
    ) {
        check(destination.mkdirs()) { "无法创建服务日志目录。" }
        val filesByComponent = mapOf(
            "xiaomi_miplay" to File(destination, "miplay.jsonl"),
            "airplay" to File(destination, "airplay.jsonl"),
            "dlna" to File(destination, "dlna.jsonl"),
        )
        val writers = filesByComponent.mapValues { (_, file) ->
            file.bufferedWriter(Charsets.UTF_8)
        }
        try {
            snapshots
                .sortedByDescending(::rotationIndex)
                .forEach { snapshot ->
                    snapshot.bufferedReader(Charsets.UTF_8).useLines { lines ->
                        lines.forEach { line ->
                            val component = runCatching {
                                JSONObject(line).optString("component")
                            }.getOrNull()
                            writers[component]?.appendLine(line)
                        }
                    }
                }
        } finally {
            writers.values.forEach { writer -> writer.close() }
        }
    }

    private fun rotationIndex(file: File): Int =
        file.name.substringAfterLast('.', missingDelimiterValue = "0")
            .toIntOrNull()
            ?: 0

    private fun writeLogcat(
        logger: FusionPlayDiagnosticLogger,
        destination: File,
    ) {
        runCatching {
            val command = mutableListOf(
                "logcat",
                "-d",
                "-v",
                "threadtime",
                "-t",
                MAXIMUM_LOGCAT_LINES.toString(),
            )
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                command += "--pid=${Process.myPid()}"
            }
            val process = ProcessBuilder(command)
                .redirectErrorStream(true)
                .start()
            destination.bufferedWriter(Charsets.UTF_8).use { writer ->
                process.inputStream.bufferedReader(Charsets.UTF_8).useLines { lines ->
                    var remaining = 1024 * 1024
                    lines.forEachIndexed { index, line ->
                        if (index < MAXIMUM_LOGCAT_LINES && remaining > 0) {
                            val cleaned = logger.sanitizeLineForExport(line)
                            val size = cleaned.toByteArray(Charsets.UTF_8).size + 1
                            if (size <= remaining) writer.appendLine(cleaned)
                            remaining -= size
                        }
                    }
                }
            }
            val exitCode = process.waitFor()
            check(exitCode == 0) { "logcat 退出码为 $exitCode。" }
        }.onFailure { error ->
            destination.writeText(
                "无法读取 Android logcat：${logger.sanitizeLineForExport(error.message.orEmpty())}",
                Charsets.UTF_8,
            )
        }
    }

    private fun zipDirectory(source: File, archive: File) {
        ZipOutputStream(BufferedOutputStream(FileOutputStream(archive))).use { output ->
            source.walkTopDown()
                .filter { it.isFile && it != archive }
                .sortedBy { it.relativeTo(source).invariantSeparatorsPath }
                .forEach { file ->
                    val entryName = file.relativeTo(source).invariantSeparatorsPath
                    output.putNextEntry(ZipEntry(entryName).apply {
                        time = file.lastModified()
                    })
                    file.inputStream().use { input -> input.copyTo(output) }
                    output.closeEntry()
                }
        }
    }

    internal fun cleanDirectory(directory: File, newest: File? = null, now: Long = System.currentTimeMillis()) {
        val files = directory.listFiles().orEmpty()
        // createShareIntent and maintenance share a lock. A staged directory
        // left by a terminated process is never needed by a shared content URI.
        files.filter { it.isDirectory && it.name.startsWith("staging-") }
            .forEach { it.deleteRecursively() }
        var retained = 0L
        val cutoff = now - ARCHIVE_RETENTION_MS
        files.filter { it.isFile && it.name.startsWith("FusionPlay-logs-") && it.extension == "zip" }
            .sortedWith(compareByDescending<File> { it == newest }.thenByDescending { it.lastModified() })
            .forEachIndexed { index, f ->
                if (f != newest && (index >= MAX_ARCHIVES || f.lastModified() < cutoff || retained + f.length() > MAX_ARCHIVE_BYTES)) {
                    f.delete()
                } else {
                    retained += f.length()
                }
            }
    }

    private fun archiveTimestamp(): String =
        SimpleDateFormat("yyyyMMdd-HHmmss", Locale.US).apply {
            timeZone = TimeZone.getTimeZone("UTC")
        }.format(Date())
}
