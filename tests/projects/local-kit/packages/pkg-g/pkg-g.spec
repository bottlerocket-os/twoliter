%global _cross_first_party 1
%undefine _debugsource_packages

Name: %{_cross_os}pkg-g
Version: 0.0
Release: 0%{?dist}
Summary: pkg-g
License: Apache-2.0 OR MIT
URL: https://github.com/bottlerocket-os/bottlerocket

Source100: pkg-g.txt

Requires: %{_cross_os}pkg-f
BuildRequires: %{_cross_os}pkg-a

%description
%{summary}.

%prep
%setup -T -c

%build

%install
mkdir -p %{buildroot}%{_cross_datadir}
install -p -m 0644 %{S:100} %{buildroot}%{_cross_datadir}/pkg-g.txt

%files
%{_cross_datadir}/pkg-g.txt
