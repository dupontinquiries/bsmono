Name:           edge
Epoch:          3
Version:        1.2.3
Release:        4.fc42
Summary:        Edge cases for depot's repodata writer
License:        MIT
URL:            http://example.invalid/edge
Group:          Testing
Vendor:         Depot Test
Packager:       Depot <depot@example.invalid>
BuildArch:      noarch
Requires:       bash >= 5.0
Requires:       (foo or bar)
Requires(post): /bin/sh
Requires(postun): /bin/sh
Requires:       /usr/bin/env
Requires:       /usr/bin/edge-tool
Provides:       edge-api = 2
Provides:       virt(edge)
Conflicts:      old-edge < 1
Obsoletes:      ancient-edge <= 0.9
Recommends:     nice-to-have
Suggests:       maybe
Supplements:    (something and other)
Enhances:       thing
%description
Multi-line description with <angles> & ampersands
and a second line.
%install
mkdir -p %{buildroot}/etc/edge %{buildroot}/usr/bin %{buildroot}/usr/share/edge %{buildroot}/var/cache/edge
echo cfg > %{buildroot}/etc/edge/edge.conf
echo '#!/bin/sh' > %{buildroot}/usr/bin/edge-tool; chmod 755 %{buildroot}/usr/bin/edge-tool
echo data > %{buildroot}/usr/share/edge/data.txt
ln -s data.txt %{buildroot}/usr/share/edge/link.txt
touch %{buildroot}/var/cache/edge/ghost.db
%files
%config(noreplace) /etc/edge/edge.conf
/usr/bin/edge-tool
%dir /usr/share/edge
/usr/share/edge/data.txt
/usr/share/edge/link.txt
%ghost /var/cache/edge/ghost.db
%changelog
* Mon Sep 01 2025 Depot <depot@example.invalid> - 3:1.2.3-4
- newest entry with "quotes" & <tags>
* Sun Aug 31 2025 Depot <depot@example.invalid> - 3:1.2.3-3
- older entry
* Sat Aug 30 2025 Someone Else <else@example.invalid> - 3:1.2.3-2
- oldest entry
