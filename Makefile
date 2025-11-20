# ------------------------------------------------------------
# Rust → RPM Makefile
# Generates .crate + rust2rpm spec only if missing.
# ------------------------------------------------------------

# Automatically read crate name + version from Cargo.toml
CRATENAME := $(shell awk -F' *= *' '/^name *=/{gsub(/"/,""); print $$2; exit}' Cargo.toml)
CRATEVER  := $(shell awk -F' *= *' '/^version *=/{gsub(/"/,""); print $$2; exit}' Cargo.toml)

CRATEFILE := $(CRATENAME)-$(CRATEVER).crate
SPECFILE  := rust-test-bd.spec


# SRPM name (expand using the spec after it exists)
SRPMFILE := $(shell \
    if [ -f "$(SPECFILE)" ]; then \
        rpmspec -q --srpm --qf '%{NAME}-%{VERSION}-%{RELEASE}.src.rpm\n' "$(SPECFILE)" | head -n1; \
    fi \
)

FEDORA_RELEASE ?= $(shell rpm -E %fedora)
MOCKCFG        ?= fedora-$(FEDORA_RELEASE)-x86_64

.PHONY: all srpm rpm mockbuild clean crate spec

all: rpm

# ------------------------------------------------------------
# Build crate tarball if missing
# ------------------------------------------------------------
$(CRATEFILE):
	cargo package --allow-dirty
	@mv target/package/$(CRATENAME)-$(CRATEVER).crate .

crate: $(CRATEFILE)

# ------------------------------------------------------------
# Generate spec file only if missing or outdated
# rust2rpm reads the crate file, so spec depends on crate
# ------------------------------------------------------------
$(SPECFILE): $(CRATEFILE)
	rust2rpm -t fedora -s -V auto test-bd

spec: $(SPECFILE)

# ------------------------------------------------------------
# Build SRPM
# ------------------------------------------------------------
srpm: $(SPECFILE)
	rpmbuild -bs \
	  --define "_sourcedir $(PWD)" \
	  --define "_specdir $(PWD)" \
	  --define "_srcrpmdir $(PWD)" \
	  $(SPECFILE)
	@echo
	@echo "Built SRPM: $(SRPMFILE)"

# ------------------------------------------------------------
# Build binary RPMs
# ------------------------------------------------------------
rpm: $(SPECFILE)
	mkdir -p RPMS
	rpmbuild -ba \
	  --define "_sourcedir $(PWD)" \
	  --define "_specdir $(PWD)" \
	  --define "_rpmdir $(PWD)/RPMS" \
	  $(SPECFILE)
	@echo
	@echo "Built RPMs in ./RPMS"

# ------------------------------------------------------------
# Mock build
# ------------------------------------------------------------
mockbuild: srpm
	mkdir -p mock-results
	@echo "Looking for SRPM file in $(SRPMFILE)"
	mock -r $(MOCKCFG) --resultdir=mock-results --rebuild $(SRPMFILE)
	@echo
	@echo "Mock build results in ./mock-results"

# ------------------------------------------------------------
# Cleanup
# ------------------------------------------------------------
clean:
	rm -rf RPMS mock-results *.src.rpm *.crate test-bd-*.tar.xz *.spec
	cargo clean

