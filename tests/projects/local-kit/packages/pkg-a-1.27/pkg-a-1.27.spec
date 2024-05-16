%global _cross_first_party 1
%undefine _debugsource_packages

Name: %{_cross_os}pkg-a
Version: 0.0
Release: 0%{?dist}
Summary: pkg-a
License: Apache-2.0 OR MIT
URL: https://github.com/bottlerocket-os/bottlerocket

Source100: pkg-a.txt

%description
%{summary}.

%prep
%setup -T -c

%build

%install
mkdir -p %{buildroot}%{_cross_datadir}
install -p -m 0644 %{S:100} %{buildroot}%{_cross_datadir}/pkg-a.txt

%files
%{_cross_datadir}/pkg-a.txt
