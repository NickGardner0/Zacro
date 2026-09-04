#!/bin/sh
set -eu

repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
profile=${1:-release}
case "$profile" in
  debug)
    cargo_args=""
    ;;
  release)
    cargo_args="--release"
    ;;
  *)
    echo "usage: $0 [debug|release]" >&2
    exit 2
    ;;
esac

version=${MACRO_VERSION:-0.1.0}
build_number=${MACRO_BUILD_NUMBER:-1}
codesign_identity=${MACRO_CODESIGN_IDENTITY:--}
notary_profile=${MACRO_NOTARY_PROFILE:-}
team_identifier=${MACRO_TEAM_IDENTIFIER:-}
update_base_url=${MACRO_UPDATE_BASE_URL:-}

if ! printf '%s' "$version" | LC_ALL=C grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.+-]+)?$'; then
  echo "MACRO_VERSION must be a JSON-safe semantic version" >&2
  exit 2
fi
if ! printf '%s' "$build_number" | LC_ALL=C grep -Eq '^[0-9]+$'; then
  echo "MACRO_BUILD_NUMBER must contain only decimal digits" >&2
  exit 2
fi
if [ -n "$team_identifier" ] && ! printf '%s' "$team_identifier" | LC_ALL=C grep -Eq '^[0-9A-Za-z]+$'; then
  echo "MACRO_TEAM_IDENTIFIER must contain only ASCII letters and digits" >&2
  exit 2
fi
if [ -n "$update_base_url" ]; then
  if ! printf '%s' "$update_base_url" | LC_ALL=C grep -Eq '^https://[^[:space:]"\\]+$'; then
    echo "MACRO_UPDATE_BASE_URL must use a JSON-safe HTTPS URL" >&2
    exit 2
  fi
fi
if [ "$codesign_identity" != "-" ] && { [ -z "$team_identifier" ] || [ -z "$update_base_url" ]; }; then
  echo "Developer ID releases require MACRO_TEAM_IDENTIFIER and MACRO_UPDATE_BASE_URL" >&2
  exit 2
fi

export MACRO_VERSION="$version"
export MACRO_BUILD_NUMBER="$build_number"
export MACRO_UPDATE_BASE_URL="$update_base_url"
export MACRO_TEAM_IDENTIFIER="$team_identifier"

cd "$repository_root"
cargo build -p macro_app $cargo_args

distribution_root="$repository_root/target/macro-gpui/$profile"
bundle_root="$distribution_root/Macro.app"
archive_path="$distribution_root/Macro-$version-macos.zip"
executable_source="$repository_root/target/$profile/macro_app"
plist_source="$repository_root/crates/macro-gpui/macro-app/resources/macos/Info.plist"
entitlements="$repository_root/crates/macro-gpui/macro-app/resources/macos/Macro.entitlements"
staging_root=$(mktemp -d "${TMPDIR:-/tmp}/macro-gpui-bundle.XXXXXX")
staging_bundle="$staging_root/Macro.app"
contents="$staging_bundle/Contents"

cleanup() {
  rm -rf "$staging_root"
}
trap cleanup EXIT HUP INT TERM

mkdir -p "$contents/MacOS" "$contents/Resources"
cp "$executable_source" "$contents/MacOS/Macro"
cp "$plist_source" "$contents/Info.plist"
chmod 755 "$contents/MacOS/Macro"

if command -v plutil >/dev/null 2>&1; then
  plutil -replace CFBundleShortVersionString -string "$version" "$contents/Info.plist"
  plutil -replace CFBundleVersion -string "$build_number" "$contents/Info.plist"
  plutil -lint "$contents/Info.plist" >/dev/null
fi

if command -v codesign >/dev/null 2>&1; then
  if [ "$codesign_identity" = "-" ]; then
    codesign --force --deep --sign - --entitlements "$entitlements" "$staging_bundle"
  else
    codesign \
      --force \
      --deep \
      --options runtime \
      --timestamp \
      --sign "$codesign_identity" \
      --entitlements "$entitlements" \
      "$staging_bundle"
  fi
  codesign --verify --deep --strict "$staging_bundle"
fi

mkdir -p "$distribution_root"
backup_bundle="$distribution_root/.Macro.app.previous"
rm -rf "$backup_bundle"
if [ -e "$bundle_root" ]; then
  mv "$bundle_root" "$backup_bundle"
fi
if mv "$staging_bundle" "$bundle_root"; then
  rm -rf "$backup_bundle"
elif [ -e "$backup_bundle" ]; then
  mv "$backup_bundle" "$bundle_root"
  exit 1
else
  exit 1
fi

if [ -n "$notary_profile" ]; then
  if [ "$codesign_identity" = "-" ]; then
    echo "MACRO_NOTARY_PROFILE requires a Developer ID MACRO_CODESIGN_IDENTITY" >&2
    exit 2
  fi
  command -v xcrun >/dev/null 2>&1 || {
    echo "xcrun is required for notarization" >&2
    exit 2
  }
  ditto -c -k --sequesterRsrc --keepParent "$bundle_root" "$archive_path"
  xcrun notarytool submit "$archive_path" --keychain-profile "$notary_profile" --wait
  xcrun stapler staple "$bundle_root"
  codesign --verify --deep --strict "$bundle_root"
  spctl --assess --type execute --verbose "$bundle_root"
fi

ditto -c -k --sequesterRsrc --keepParent "$bundle_root" "$archive_path"
archive_sha256=$(shasum -a 256 "$archive_path" | awk '{print $1}')
archive_size=$(stat -f '%z' "$archive_path")
manifest_path="$distribution_root/update-manifest.json"
archive_url="${update_base_url%/}/$(basename "$archive_path")"
if [ -z "$update_base_url" ]; then
  archive_url=$(basename "$archive_path")
fi
if [ -n "$team_identifier" ]; then
  team_identifier_json="\"$team_identifier\""
else
  team_identifier_json=null
fi
printf '%s\n' \
  '{' \
  "  \"schema_version\": 2," \
  "  \"version\": \"$version\"," \
  "  \"build\": \"$build_number\"," \
  "  \"bundle_identifier\": \"com.macro.desktop.gpui\"," \
  "  \"url\": \"$archive_url\"," \
  "  \"sha256\": \"$archive_sha256\"," \
  "  \"size\": $archive_size," \
  "  \"team_identifier\": $team_identifier_json," \
  '  "minimum_system_version": "12.0"' \
  '}' >"$manifest_path"

echo "$bundle_root"
echo "$archive_path"
echo "$manifest_path"
