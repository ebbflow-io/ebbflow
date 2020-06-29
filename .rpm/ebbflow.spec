%define __spec_install_post %{nil}
%define __os_install_post %{_dbpath}/brp-compress
%define debug_package %{nil}

Name: ebbflow
Summary: The on-host executable client for interacting with Ebbflow.io
Version: @@VERSION@@
Release: @@RELEASE@@
License: University of Illinois/NCSA Open Source License Copyright (c) All rights reserved.
Group: System Environment/Daemons
Source0: %{name}-%{version}.tar.gz
URL: https://ebbflow.io

BuildRoot: %{_tmppath}/%{name}-%{version}-%{release}-root
BuildRequires: systemd

Requires(post): systemd
Requires(preun): systemd
Requires(postun): systemd

%description
%{summary}

%prep
%setup -q

%install
rm -rf %{buildroot}
mkdir -p %{buildroot}
cp -a * %{buildroot}

%clean
rm -rf %{buildroot}

%systemd_post ebbflowd.service

%preun
%systemd_preun ebbflowd.service

%postun
%systemd_postun_with_restart ebbflowd.service

%files
%defattr(-,root,root,-)
%{_bindir}/*
%{_sbindir}/*
%{_unitdir}/ebbflowd.service
%attr(0644,root,root) %config(noreplace) /etc/ebbflow/config.yaml
%attr(0644,root,root) %config(noreplace) /etc/ebbflow/.daemonaddr
%attr(0600,root,root) %config(noreplace) /etc/ebbflow/host.key
