%global _cross_first_party 1
%undefine _debugsource_packages

Name: %{_cross_os}hello-go
Version: 0.0
Release: 0%{?dist}
Summary: hello-go
License: Apache-2.0 OR MIT
URL: https://github.com/bottlerocket-os/bottlerocket

# sources < 100: misc

# 1xx sources: systemd units
Source103: hello-go.service
Source104: hello-go.timer

BuildRequires: %{_cross_os}glibc-devel
Requires: %{_cross_os}hello-go

%description
%{summary}.

%prep
%setup -T -c
cp -r %{_builddir}/sources/hello-go/* .

%build
%set_cross_go_flags
go build -buildmode=pie -ldflags="${GOLDFLAGS}" -o hello-go ./cmd/hello-go

%install
install -d %{buildroot}%{_cross_bindir}
install -p -m 0755 hello-go %{buildroot}%{_cross_bindir}
 
install -d %{buildroot}%{_cross_unitdir}
install -p -m 0644 \
  %{S:103} %{S:104} \
  %{buildroot}%{_cross_unitdir}

%files
%{_cross_bindir}/hello-go
%{_cross_unitdir}/hello-go.service
%{_cross_unitdir}/hello-go.timer
