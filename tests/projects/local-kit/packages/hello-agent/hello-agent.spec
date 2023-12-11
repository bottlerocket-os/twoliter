%global _cross_first_party 1
%undefine _debugsource_packages

Name: %{_cross_os}hello-agent
Version: 0.0
Release: 0%{?dist}
Summary: Hello-agent
License: Apache-2.0 OR MIT
URL: https://github.com/bottlerocket-os/bottlerocket

# sources < 100: misc

# 1xx sources: systemd units
Source103: hello-agent.service
Source104: hello-agent.timer

BuildRequires: %{_cross_os}glibc-devel

%description
%{summary}.

%prep
%setup -T -c
%cargo_prep

%build
mkdir bin

%cargo_build_static --manifest-path %{_builddir}/sources/Cargo.toml \
    -p hello-agent

%install
install -d %{buildroot}%{_cross_bindir}
install -p -m 0755 ${HOME}/.cache/.static/%{__cargo_target_static}/release/hello-agent %{buildroot}%{_cross_bindir}
 
install -d %{buildroot}%{_cross_unitdir}
install -p -m 0644 \
  %{S:103} %{S:104} \
  %{buildroot}%{_cross_unitdir}

%files
%{_cross_bindir}/hello-agent
%{_cross_unitdir}/hello-agent.service
%{_cross_unitdir}/hello-agent.timer
