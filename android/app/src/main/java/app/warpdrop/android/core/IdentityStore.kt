package app.warpdrop.android.core

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Log
import java.io.File
import java.nio.ByteBuffer
import java.security.KeyStore
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Keeps the 32-byte Ed25519 identity seed encrypted at rest with an AES-256-GCM
 * key that never leaves the Android Keystore. The ciphertext lives in
 * `noBackupFilesDir`, so it is never copied off the device.
 *
 * File format (v1): `[0x01][iv length][iv][ciphertext ‖ tag]`.
 */
object IdentityStore {
    private const val TAG = "IdentityStore"
    private const val KEYSTORE = "AndroidKeyStore"
    private const val KEY_ALIAS = "warpdrop.identity.v1"
    private const val FILE_NAME = "identity.v1.bin"
    private const val TRANSFORMATION = "AES/GCM/NoPadding"
    private const val TAG_BITS = 128
    private const val VERSION: Byte = 1
    private const val SEED_LEN = 32

    @Synchronized
    fun loadOrCreateSeed(context: Context): ByteArray {
        val file = File(context.noBackupFilesDir, FILE_NAME)
        if (file.exists()) {
            try {
                val seed = decrypt(file.readBytes())
                if (seed.size == SEED_LEN) return seed
                Log.w(TAG, "stored seed has the wrong length; regenerating")
            } catch (e: Exception) {
                // Keystore key lost (e.g. lock-screen reset on some OEMs): a new
                // identity is the only option. Trusted devices must re-link.
                Log.w(TAG, "cannot decrypt the identity seed; regenerating", e)
            }
        }
        val seed = ByteArray(SEED_LEN).also { SecureRandom().nextBytes(it) }
        val tmp = File(file.parentFile, "$FILE_NAME.tmp")
        tmp.writeBytes(encrypt(seed))
        if (!tmp.renameTo(file)) {
            file.delete()
            check(tmp.renameTo(file)) { "cannot persist the identity seed" }
        }
        return seed
    }

    private fun key(): SecretKey {
        val ks = KeyStore.getInstance(KEYSTORE).apply { load(null) }
        (ks.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
        generator.init(
            KeyGenParameterSpec.Builder(KEY_ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .setRandomizedEncryptionRequired(true)
                .build(),
        )
        return generator.generateKey()
    }

    private fun encrypt(plain: ByteArray): ByteArray {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val iv = cipher.iv
        val ct = cipher.doFinal(plain)
        return ByteBuffer.allocate(2 + iv.size + ct.size)
            .put(VERSION).put(iv.size.toByte()).put(iv).put(ct).array()
    }

    private fun decrypt(blob: ByteArray): ByteArray {
        val buf = ByteBuffer.wrap(blob)
        require(buf.get() == VERSION) { "unknown identity file version" }
        val iv = ByteArray(buf.get().toInt() and 0xFF).also { buf.get(it) }
        val ct = ByteArray(buf.remaining()).also { buf.get(it) }
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(TAG_BITS, iv))
        return cipher.doFinal(ct)
    }
}
