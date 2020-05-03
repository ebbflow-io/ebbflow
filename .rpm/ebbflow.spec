%define __spec_install_post %{nil}
%define __os_install_post %{_dbpath}/brp-compress
%define debug_package %{nil}

Name: ebbflow
Summary: Proxies Ebbflow connections to your server.
Version: @@VERSION@@
Release: @@RELEASE@@
License: University of Illinois/NCSA Open Source License Copyright (c) All rights reserved.
Group: Applications/System
Source0: %{name}-%{version}.tar.gz
URL: https://ebbflow.io

BuildRoot: %{_tmppath}/%{name}-%{version}-%{release}-root

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

%files
%defattr(-,root,root,-)
%{_bindir}/*
