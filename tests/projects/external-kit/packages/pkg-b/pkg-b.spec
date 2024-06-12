%global _cross_first_party 1
%undefine _debugsource_packages

Name: %{_cross_os}pkg-b
Version: 0.0
Release: 0%{?dist}
Summary: pkg-b
License: Apache-2.0 OR MIT
URL: https://github.com/bottlerocket-os/bottlerocket

Source100: pkg-b.txt

# This package comes from the core-kit defined in the local-kit project
BuildRequires: %{_cross_os}pkg-a

%description
%{summary}.

%prep
%setup -T -c

%build

%install
mkdir -p %{buildroot}%{_cross_datadir}
install -p -m 0644 %{S:100} %{buildroot}%{_cross_datadir}/pkg-b.txt

%files
%{_cross_datadir}/pkg-b.txt
