-- Gate-level simulation of a successful run's netlist (see gls.rs): the
-- testbenches that match the build's parameters, rerun on the gates.
ALTER TABLE jobs ADD COLUMN gls JSONB;
