Name:           redbackup
Version:        %{redbackup_version}
Release:        1
Summary:        Deduplicating backup engine and user daemon
License:        LicenseRef-NotDeclared
Source0:        %{name}-%{version}.tar.gz
ExclusiveArch:  x86_64
BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  gcc
BuildRequires:  pkgconfig(sqlite3)
BuildRequires:  pkgconfig(systemd)
BuildRequires:  systemd-rpm-macros
Requires:       systemd

%description
Creates and restores encrypted snapshots of local files.

%prep
%setup -q

%build
cargo build --release --locked --bins --target-dir cargo-target

%check
cargo test --locked --workspace --target-dir cargo-target

%install
install -Dm755 cargo-target/release/redbackup %{buildroot}%{_bindir}/redbackup
install -Dm755 cargo-target/release/redbackupd %{buildroot}%{_bindir}/redbackupd
install -Dm644 packaging/systemd/redbackupd.service %{buildroot}%{_userunitdir}/redbackupd.service
sed -i 's|^ExecStart=.*|ExecStart=%{_bindir}/redbackupd|' %{buildroot}%{_userunitdir}/redbackupd.service

%files
%{_bindir}/redbackup
%{_bindir}/redbackupd
%{_userunitdir}/redbackupd.service
