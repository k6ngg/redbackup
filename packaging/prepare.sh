#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
    echo "Usage: $0 deb|rpm NEW_BUILD_DIRECTORY" >&2
    exit 2
fi

format=$1
case "$format" in
    deb|rpm) ;;
    *) echo "Unknown format: $format" >&2; exit 2 ;;
esac

mkdir -- "$2"
output=$(cd -- "$2" && pwd)
cd "$(dirname "$0")/.."
version=$(sed -n '/^\[package\]$/,/^\[/s/^version = "\([0-9][0-9.]*\)"$/\1/p' Cargo.toml)
if [ -z "$version" ]; then
    echo "Expected a numeric release version in Cargo.toml" >&2
    exit 1
fi

source="redbackup-$version"
mkdir "$output/$source"
cp -R Cargo.toml Cargo.lock README.md src tests packaging "$output/$source/"

case "$format" in
    deb)
        cp -R packaging/debian "$output/$source/debian"
        printf 'redbackup (%s-1) unstable; urgency=medium\n\n  * Local package build.\n\n -- redbackup local builds <redbackup@localhost>  %s\n' \
            "$version" "$(LC_ALL=C date -Ru)" > "$output/$source/debian/changelog"
        printf '%s\n' "$output/$source"
        ;;
    rpm)
        mkdir "$output/SOURCES"
        tar -czf "$output/SOURCES/$source.tar.gz" -C "$output" "$source"
        printf '%s\n' "$version"
        ;;
esac
