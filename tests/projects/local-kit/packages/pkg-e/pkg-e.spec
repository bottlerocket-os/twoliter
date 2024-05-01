%global _cross_first_party 1
%undefine _debugsource_packages

Name: %{_cross_os}pkg-e
Version: 0.0
Release: 0%{?dist}
Summary: pkg-e
License: Apache-2.0 OR MIT
URL: https://github.com/bottlerocket-os/bottlerocket

Source100: pkg-e.txt

Requires: %{_cross_os}pkg-c
BuildRequires: %{_cross_os}pkg-a
BuildRequires: %{_cross_os}pkg-b
BuildRequires: %{_cross_os}pkg-c
BuildRequires: %{_cross_os}pkg-d

%description
%{summary}.

%prep
%setup -T -c

%build

%install
mkdir -p %{buildroot}%{_cross_datadir}
install -p -m 0644 %{S:100} %{buildroot}%{_cross_datadir}/pkg-e.txt

%files
%{_cross_datadir}/pkg-e.txt
