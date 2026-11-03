#!/usr/bin/env bash
# Activates the MSVC target alongside the self-contained gnu toolchain.
#
#   source scripts/env-msvc.sh
#
# The gnu toolchain is the default (scripts/env.sh) because it needs no Visual Studio.
# This one is for work that genuinely needs MSVC — Tauri, whose build script invokes the
# resource compiler and whose WebView2 import libraries are MSVC-shaped.
#
# Everything is discovered, not hard-coded: the Visual Studio install path comes from
# vswhere, and the Windows SDK path from the SDK's own registry-free layout on whichever
# drive it was installed to.

O824_MSVC_VS="${O824_MSVC_VS:-D:/VS/BuildTools}"
O824_MSVC_SDK="${O824_MSVC_SDK:-D:/Windows Kits/10}"

if [ ! -d "$O824_MSVC_VS" ]; then
  echo "env-msvc: no Visual Studio build tools at $O824_MSVC_VS" >&2
  return 1 2>/dev/null || exit 1
fi

# The newest MSVC toolset.
O824_MSVC_TOOLS="$(ls -d "$O824_MSVC_VS"/VC/Tools/MSVC/*/ 2>/dev/null | sort -V | tail -1)"
O824_MSVC_TOOLS="${O824_MSVC_TOOLS%/}"
if [ -z "$O824_MSVC_TOOLS" ]; then
  echo "env-msvc: no MSVC toolset under $O824_MSVC_VS" >&2
  return 1 2>/dev/null || exit 1
fi

# The newest Windows SDK version that has both headers and libraries.
O824_MSVC_SDK_VER=""
for candidate in $(ls -d "$O824_MSVC_SDK"/Lib/*/ 2>/dev/null | sort -V -r); do
  version="$(basename "$candidate")"
  if [ -d "$O824_MSVC_SDK/Include/$version/um" ] && [ -d "$O824_MSVC_SDK/Lib/$version/um/x64" ]; then
    O824_MSVC_SDK_VER="$version"
    break
  fi
done
if [ -z "$O824_MSVC_SDK_VER" ]; then
  echo "env-msvc: no usable Windows SDK under $O824_MSVC_SDK" >&2
  return 1 2>/dev/null || exit 1
fi

export O824_MSVC_VS O824_MSVC_SDK O824_MSVC_TOOLS O824_MSVC_SDK_VER

# rustc calls link.exe by name, so the toolset's bin directory has to be first: the
# Windows loader would otherwise find the system's ancient link.exe.
export PATH="$O824_MSVC_TOOLS/bin/Hostx64/x64:$PATH"

export INCLUDE="$O824_MSVC_TOOLS/include;$O824_MSVC_SDK/Include/$O824_MSVC_SDK_VER/ucrt;$O824_MSVC_SDK/Include/$O824_MSVC_SDK_VER/shared;$O824_MSVC_SDK/Include/$O824_MSVC_SDK_VER/um;$O824_MSVC_SDK/Include/$O824_MSVC_SDK_VER/winrt"
export LIB="$O824_MSVC_TOOLS/lib/x64;$O824_MSVC_SDK/Lib/$O824_MSVC_SDK_VER/ucrt/x64;$O824_MSVC_SDK/Lib/$O824_MSVC_SDK_VER/um/x64"

echo "env-msvc: msvc $(basename "$O824_MSVC_TOOLS"), sdk $O824_MSVC_SDK_VER"
