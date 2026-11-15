# JNA + UniFFI: the bindings look up native symbols and callback structures by name.
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-keep class uniffi.warp.** { *; }
-dontwarn java.awt.**

# ML Kit (QR scanner): its components are discovered by reflection through
# ComponentRegistrar classes; R8 must keep their no-arg constructors or the
# barcode scanner fails at runtime in release builds.
-keep class * implements com.google.firebase.components.ComponentRegistrar { public <init>(); }
-keep class com.google.mlkit.**.*Registrar { public <init>(); }
