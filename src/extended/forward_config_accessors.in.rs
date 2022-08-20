// Because Rust doesn't really support scoped macros, this file gets to be textually included.

macro_rules! forward_config_accessors {
	($type:ty, $getter:ident, $mut_getter:ident, $setter:ident, $doc:literal) => {
		#[doc = "Gets "]
		#[doc = $doc]
		pub fn $getter(&self) -> &$type {
			self.config.$getter()
		}

		#[doc = "Gets a mutable reference to "]
		#[doc = $doc]
		pub fn $mut_getter(&mut self) -> &mut $type {
			self.config.$mut_getter()
		}
		
		#[doc = "Sets "]
		#[doc = $doc]
		#[doc = "\n\nReturns `self` for easy chaining."]
		pub fn $setter(&mut self, value: $type) -> &mut Self {
			self.config.$setter(value);
			self
		}
	};
}