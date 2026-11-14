package com.fusionplay.android.update

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.core.content.FileProvider
import java.io.File

/** 拉起系统安装器。 */
sealed interface InstallResult {
    data object Started : InstallResult
    data class PermissionRequired(val message: String) : InstallResult
    data class Failure(val message: String) : InstallResult
}

class UpdateInstaller(private val context: Context) {
    fun install(apk: File): InstallResult {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O &&
            !context.packageManager.canRequestPackageInstalls()
        ) {
            val settings = Intent(
                Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                Uri.parse("package:${context.packageName}"),
            ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            runCatching { context.startActivity(settings) }
            return InstallResult.PermissionRequired(
                "请在系统设置中允许 FusionPlay 安装未知应用，然后重新点击安装",
            )
        }
        return try {
            val uri = FileProvider.getUriForFile(
                context,
                "${context.packageName}.logprovider",
                apk,
            )
            val intent = Intent(Intent.ACTION_VIEW)
                .setDataAndType(uri, "application/vnd.android.package-archive")
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            context.startActivity(intent)
            InstallResult.Started
        } catch (error: Exception) {
            InstallResult.Failure(error.message ?: "无法启动系统安装器")
        }
    }
}
