%global _cross_first_party 1
%undefine _debugsource_packages

Name: %{_cross_os}inner-pkg
Version: 0.0
Release: 0%{?dist}
Summary: A trivial sibling package alongside the host/guest variants in the fixture
License: Apache-2.0 OR MIT
URL: https://github.com/bottlerocket-os/bottlerocket

%description
%{summary}.

%prep
%setup -T -c

%build

%install
mkdir -p %{buildroot}%{_cross_datadir}/inner-pkg
echo "hello from inner-pkg" > %{buildroot}%{_cross_datadir}/inner-pkg/hello

%files
%dir %{_cross_datadir}/inner-pkg
%{_cross_datadir}/inner-pkg/hello
