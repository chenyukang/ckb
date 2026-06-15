#!/bin/bash
set -eu

GIT_TAG_NAME="${GIT_TAG_NAME:-"$(git describe)"}"
CKB_CLI_VERSION="${CKB_CLI_VERSION:-"$GIT_TAG_NAME"}"
if [ -z "${REL_PKG:-}" ]; then
  if [ "$(uname)" = Darwin ]; then
    REL_PKG=x86_64-apple-darwin.zip
  else
    REL_PKG=x86_64-unknown-linux-gnu.tar.gz
  fi
fi

ckb_cli_sha256() {
  case "${CKB_CLI_VERSION}_${CKB_CLI_REL_PKG}" in
    v2.0.0_aarch64-apple-darwin.zip) echo "e19e0fc8228e3cc9eba5e4e3506f2559a8321cf472a9f5392bc6dc4093308d06" ;;
    v2.0.0_aarch64-unknown-linux-gnu.tar.gz) echo "ddc3b3e08724983ee08e1b96336550e016ba0bc8156ea68e3bbe1f0f2f33f9c2" ;;
    v2.0.0_x86_64-apple-darwin.zip) echo "47ab71ba16ebab79bf312e59eb547b66348a9ecf59b136909154473fb9551846" ;;
    v2.0.0_x86_64-unknown-centos-gnu.tar.gz) echo "87fb1050f77b1677460f0bd2985eac5585e6f8d0aa8fabca40293121d8116cbd" ;;
    v2.0.0_x86_64-unknown-linux-gnu.tar.gz) echo "f61ea734a50f4ce91c058b92175950bcb2042847304480123f487d64214d3082" ;;
    *) return 1 ;;
  esac
}

verify_sha256() {
  expected="$1"
  file="$2"
  if command -v sha256sum >/dev/null 2>&1; then
    printf '%s  %s\n' "$expected" "$file" | sha256sum -c -
  else
    actual="$(shasum -a 256 "$file" | awk '{print $1}')"
    if [ "$actual" != "$expected" ]; then
      echo "sha256 mismatch for $file: expected $expected, got $actual"
      exit 1
    fi
  fi
}

PKG_NAME="ckb_${GIT_TAG_NAME}_${REL_PKG%%.*}"
ARCHIVE_NAME="ckb_${GIT_TAG_NAME}_${REL_PKG}"
echo "ARCHIVE_NAME=$ARCHIVE_NAME"

rm -rf releases
mkdir releases
mkdir "releases/$PKG_NAME"
cp "$1" "releases/$PKG_NAME"
cp README.md CHANGELOG.md COPYING "releases/$PKG_NAME"
cp -R devtools/init "releases/$PKG_NAME"
cp -R docs "releases/$PKG_NAME"
cp rpc/README.md "releases/$PKG_NAME/docs/rpc.md"

if [ ! "${SKIP_CKB_CLI:-false}" == "true" ]; then
  CKB_CLI_REL_PKG="$(echo "$REL_PKG" | sed 's/-portable//')"
  CKB_CLI_ARCHIVE="ckb-cli_${CKB_CLI_VERSION}_${CKB_CLI_REL_PKG}"
  CKB_CLI_SHA256="$(ckb_cli_sha256)"
  curl -fLO "https://github.com/nervosnetwork/ckb-cli/releases/download/${CKB_CLI_VERSION}/${CKB_CLI_ARCHIVE}"
  verify_sha256 "$CKB_CLI_SHA256" "$CKB_CLI_ARCHIVE"
  if [ "${CKB_CLI_REL_PKG##*.}" = "zip" ]; then
    unzip "$CKB_CLI_ARCHIVE"
  else
    tar -xzf "$CKB_CLI_ARCHIVE"
  fi
  mv "ckb-cli_${CKB_CLI_VERSION}_${CKB_CLI_REL_PKG%%.*}/ckb-cli" "releases/$PKG_NAME/ckb-cli"
fi

pushd releases
if [ "${REL_PKG#*.}" = "tar.gz" ]; then
  tar -czf $PKG_NAME.tar.gz $PKG_NAME
else
  zip -r $PKG_NAME.zip $PKG_NAME
fi
if [ -n "${GPG_SIGNER:-}" ]; then
  gpg -u "$GPG_SIGNER" -ab "$ARCHIVE_NAME"
fi
popd
