package expo.modules.anzen

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import android.view.WindowManager
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import java.io.File
import java.security.KeyStore
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

class AnzenRuntimeModule : Module() {
  companion object { init { System.loadLibrary("anzen_jni") } }
  private external fun executeRust(root: String, role: String, key: ByteArray, settings: String, command: String): ByteArray
  private external fun watchRust(settings: String, request: String): ByteArray

  override fun definition() = ModuleDefinition {
    Name("AnzenRuntime")
    // A wallet's screens stay out of the recent-apps picture, screenshots and screen recordings.
    // Android takes that picture before the app hears it is leaving, so covering the screen from
    // JavaScript (as on iOS) would come too late.
    OnActivityEntersForeground {
      appContext.currentActivity?.let { activity ->
        activity.runOnUiThread { activity.window.setFlags(WindowManager.LayoutParams.FLAG_SECURE, WindowManager.LayoutParams.FLAG_SECURE) }
      }
    }
    AsyncFunction("execute") { role: String, command: String, settings: String ->
      executeRequest(role, command, settings)
    }
    // Live updates run on Rust's own thread; this call only starts, updates or reads them, so it
    // is not synchronized with executeRequest and never waits behind a command.
    Function("watch") { settings: String, request: String ->
      watchRust(settings, request).toString(Charsets.UTF_8)
    }
  }

  @Synchronized private fun executeRequest(role: String, command: String, settings: String): String {
    require(role == "wallet" || role == "signer") { "Invalid device role" }
    val context = requireNotNull(appContext.reactContext) { "No Android app context" }
    val root = File(context.noBackupFilesDir, "AnzenDevices")
    check(root.isDirectory || root.mkdirs()) { "Cannot open protected app storage" }
    val key = storageKey(root, role)
    try { return executeRust(root.absolutePath, role, key, settings, command).toString(Charsets.UTF_8) }
    finally { key.fill(0) }
  }

  // AndroidKeyStore holds a non-exportable wrapping key; only the random data-encryption key
  // enters Rust memory. Both the ciphertext and app state are excluded from Android backups.
  // A role's devices live at root/<network>/<role>, or at root/<role> from before networks were
  // a setting.
  private fun deviceExists(root: File, role: String): Boolean =
    (listOf(role) + listOf("signet", "regtest", "bitcoin").map { "$it/$role" }).any { File(root, it).exists() }

  private fun storageKey(root: File, role: String): ByteArray {
    val alias = "app.anzen.storage.v1.$role"
    val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
    val file = AtomicFile(File(root, "$role.key"))
    if (!store.containsAlias(alias)) {
      check(!file.baseFile.exists() && !deviceExists(root, role)) { "Device storage key is missing; restore from a recovery backup" }
      val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
      generator.init(KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
        .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
        .setKeySize(256).build())
      generator.generateKey()
    }
    val wrapping = store.getKey(alias, null) as SecretKey
    if (file.baseFile.exists()) {
      val bytes = file.readFully()
      check(bytes.size == 12 + 32 + 16) { "Invalid wrapped storage key" }
      val cipher = Cipher.getInstance("AES/GCM/NoPadding")
      cipher.init(Cipher.DECRYPT_MODE, wrapping, GCMParameterSpec(128, bytes.copyOfRange(0, 12)))
      cipher.updateAAD(alias.toByteArray(Charsets.UTF_8))
      return cipher.doFinal(bytes.copyOfRange(12, bytes.size))
    }
    check(!deviceExists(root, role)) { "Wrapped storage key missing; refusing to replace it" }
    val key = ByteArray(32).also { SecureRandom().nextBytes(it) }
    val cipher = Cipher.getInstance("AES/GCM/NoPadding")
    cipher.init(Cipher.ENCRYPT_MODE, wrapping)
    cipher.updateAAD(alias.toByteArray(Charsets.UTF_8))
    val stream = file.startWrite()
    try {
      stream.write(cipher.iv + cipher.doFinal(key))
      file.finishWrite(stream)
    } catch (error: Throwable) {
      file.failWrite(stream)
      key.fill(0)
      throw error
    }
    return key
  }
}
