%global cross_generate_attribution %{nil}

Name: %{_cross_os}builder-group
Version: 1.0
Release: 1%{?dist}
Summary: Provides group(builder) for kernel-devel subpackages

License: Apache-2.0 OR MIT
URL: https://github.com/bottlerocket-os/twoliter

# This is a dummy package that provides `builder(group)`. RPM 6.0 introduced
# automatic user() and group() dependency generation from file ownership changes
# declared in `%files` sections. In earlier versions the dependencies were weak
# so they were ignored. In RPM 6.0 they are Required.

Provides: group(builder)

%description
%{summary}.

%prep

%build

%install

%files

%changelog
