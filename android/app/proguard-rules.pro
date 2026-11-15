# JNA + UniFFI: the bindings look up native symbols and callback structures by name.
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-keep class uniffi.warp.** { *; }
-dontwarn java.awt.**
