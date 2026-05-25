# Unified test surface for the IDML renderer + canvas.
#
#   make fidelity          # everything (cargo, native renderer, canvas)
#   make fidelity-rust     # cargo unit tests across the workspace
#   make fidelity-native   # InDesign-PDF vs native renderer (idml-inspect)
#   make fidelity-canvas   # InDesign-PDF vs web canvas (Playwright)
#
# Each target is independent — pick the smallest one that covers the
# layer you touched.

.PHONY: fidelity fidelity-rust fidelity-native fidelity-canvas

fidelity: fidelity-rust fidelity-native fidelity-canvas

fidelity-rust:
	cargo test --workspace

fidelity-native:
	bash corpus/envato/test.sh

fidelity-canvas:
	cd apps/canvas && npm run test:fidelity
