import groovy.json.JsonSlurper
import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "app.warpdrop.android"
    compileSdk = libs.versions.androidCompileSdk.get().toInt()

    defaultConfig {
        applicationId = "app.warpdrop.android"
        minSdk = libs.versions.androidMinSdk.get().toInt()
        targetSdk = libs.versions.androidTargetSdk.get().toInt()
        // CI passes the tag version and run number; local builds keep the defaults.
        versionCode = System.getenv("WARPDROP_VERSION_CODE")?.toIntOrNull() ?: 1
        versionName = System.getenv("WARPDROP_VERSION") ?: "0.1.0"
        ndk { abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64") }
    }

    // Release signing. CI: WARPDROP_KEYSTORE_* environment (repository secrets).
    // Local: ~/.warpdrop-keys/keystore.properties, restored from the keys vault
    // (`./vault get warpdrop/keystore.properties …`). Without either, release
    // builds are unsigned and CI publishes the debug APK instead.
    val localProps = Properties().apply {
        val f = File(System.getProperty("user.home"), ".warpdrop-keys/keystore.properties")
        if (f.isFile) f.inputStream().use { load(it) }
    }
    fun signing(env: String, prop: String): String? = System.getenv(env) ?: localProps.getProperty(prop)
    val keystore = signing("WARPDROP_KEYSTORE_FILE", "storeFile")?.let { file(it) }?.takeIf { it.exists() }
    signingConfigs {
        if (keystore != null) {
            create("release") {
                storeFile = keystore
                storePassword = signing("WARPDROP_KEYSTORE_PASSWORD", "storePassword")
                keyAlias = signing("WARPDROP_KEY_ALIAS", "keyAlias")
                keyPassword = signing("WARPDROP_KEY_PASSWORD", "keyPassword")
            }
        }
    }

    buildTypes {
        release {
            if (keystore != null) signingConfig = signingConfigs.getByName("release")
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    packaging {
        // libwarp.so is prebuilt; keep it uncompressed and page-aligned.
        jniLibs.useLegacyPackaging = false
    }

    androidResources {
        @Suppress("UnstableApiUsage")
        generateLocaleConfig = true
    }

    // Per-app language switching needs every locale in the base APK.
    bundle {
        language { enableSplit = false }
    }

    lint {
        // The UniFFI bindings are generated; lint findings there are not actionable.
        lintConfig = file("lint.xml")
    }
}

kotlin {
    compilerOptions {
        freeCompilerArgs.add("-opt-in=kotlin.ExperimentalUnsignedTypes")
    }
}

dependencies {
    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.appcompat)
    implementation(libs.androidx.documentfile)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.lifecycle.process)
    implementation(libs.androidx.lifecycle.service)
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.graphics)
    implementation(libs.androidx.compose.foundation)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.material.icons)
    implementation(libs.androidx.compose.ui.tooling.preview)
    debugImplementation(libs.androidx.compose.ui.tooling)
    implementation(libs.androidx.camera.camera2)
    implementation(libs.androidx.camera.lifecycle)
    implementation(libs.androidx.camera.view)
    implementation(libs.mlkit.barcode.scanning)
    implementation(libs.jna) { artifact { type = "aar" } }
}

// ---------------------------------------------------------------------------
// Localized strings: i18n/strings.json (canonical, shared by every platform)
// → res/values/strings.xml + res/values-it/strings.xml. Runs before every
// build so the Android copy can never drift from the JSON.
// ---------------------------------------------------------------------------
abstract class SyncStringsTask : DefaultTask() {
    @get:InputFile
    abstract val source: RegularFileProperty

    // Writes into src/main/res (the files are committed so IDEs and lint see
    // them); left untracked so resource-reading tasks need no extra wiring.
    @get:Internal
    abstract val resDir: DirectoryProperty

    @TaskAction
    fun generate() {
        @Suppress("UNCHECKED_CAST")
        val json = JsonSlurper().parse(source.get().asFile) as Map<String, Any?>
        val entries = json.filterKeys { !it.startsWith("_") }.mapValues {
            @Suppress("UNCHECKED_CAST")
            it.value as Map<String, String>
        }
        for ((lang, dir) in listOf("en" to "values", "it" to "values-it")) {
            val sb = StringBuilder()
            sb.append("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n")
            sb.append("<!-- GENERATED from i18n/strings.json by :app:syncStrings. Do not edit. -->\n")
            sb.append("<resources>\n")
            for ((key, values) in entries) {
                val text = values[lang] ?: values.getValue("en")
                val translatable = if (lang == "en" && key == "app_name") " translatable=\"false\"" else ""
                if (lang == "it" && key == "app_name") continue
                sb.append("    <string name=\"").append(key).append('"').append(translatable).append('>')
                    .append(escape(text)).append("</string>\n")
            }
            sb.append("</resources>\n")
            val out = resDir.get().asFile.resolve("$dir/strings.xml")
            out.parentFile.mkdirs()
            val content = sb.toString()
            if (!out.exists() || out.readText() != content) out.writeText(content)
        }
    }

    private fun escape(s: String): String {
        var r = s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
            .replace("\\", "\\\\").replace("'", "\\'").replace("\"", "\\\"")
            .replace("\n", "\\n")
        if (r.startsWith("@") || r.startsWith("?")) r = "\\" + r
        // {0} → %1$s, {1} → %2$s …
        r = Regex("\\{(\\d+)\\}").replace(r) { m -> "%" + (m.groupValues[1].toInt() + 1) + "\$s" }
        return r
    }
}

val syncStrings = tasks.register<SyncStringsTask>("syncStrings") {
    source.set(rootProject.layout.projectDirectory.file("../i18n/strings.json"))
    resDir.set(layout.projectDirectory.dir("src/main/res"))
    doNotTrackState("Generates committed sources in src/main/res")
}
tasks.named("preBuild") { dependsOn(syncStrings) }
