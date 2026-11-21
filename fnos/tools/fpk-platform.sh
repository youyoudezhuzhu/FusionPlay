# Sourced by the native fnOS build and package tests.
case "$(uname -m)" in
    x86_64)  FPK_PLATFORM=x86; FPK_FILENAME=fusionplay-fnos.fpk ;;
    aarch64) FPK_PLATFORM=arm; FPK_FILENAME=fusionplay-fnos-arm64.fpk ;;
    *) echo "Unsupported fnOS build architecture: $(uname -m)" >&2; return 1 ;;
esac
