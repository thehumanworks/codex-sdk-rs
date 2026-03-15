.PHONY: bump-version update-sdk-release-metadata publish-crate

bump-version:
	@test -n "$(VERSION)" || (echo "VERSION is required" && exit 1)
	@test -n "$(MANIFEST)" || (echo "MANIFEST is required" && exit 1)
	python3 scripts/bump-package-version.py "$(MANIFEST)" "$(VERSION)"

update-sdk-release-metadata:
	@test -n "$(VERSION)" || (echo "VERSION is required" && exit 1)
	python3 scripts/update-sdk-release-metadata.py "$(VERSION)"

publish-crate:
	@test -n "$(CRATE)" || (echo "CRATE is required" && exit 1)
	cargo publish -p "$(CRATE)" $(ARGS)
