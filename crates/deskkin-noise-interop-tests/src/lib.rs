#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    use deskkin_protocol::{Message, PRELUDE, authentication_string};
    use snow::params::{CipherChoice, DHChoice, HashChoice};
    use snow::resolvers::{CryptoResolver, DefaultResolver};
    use snow::types::{Cipher, Dh, Hash, Random};

    const PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";

    struct DeviceRandom;

    impl Random for DeviceRandom {
        fn try_fill_bytes(&mut self, destination: &mut [u8]) -> Result<(), snow::Error> {
            destination.fill(0x77);
            Ok(())
        }
    }

    struct DeviceResolver(DefaultResolver);

    impl CryptoResolver for DeviceResolver {
        fn resolve_rng(&self) -> Option<Box<dyn Random>> {
            Some(Box::new(DeviceRandom))
        }

        fn resolve_dh(&self, choice: &DHChoice) -> Option<Box<dyn Dh>> {
            self.0.resolve_dh(choice)
        }

        fn resolve_hash(&self, choice: &HashChoice) -> Option<Box<dyn Hash>> {
            self.0.resolve_hash(choice)
        }

        fn resolve_cipher(&self, choice: &CipherChoice) -> Option<Box<dyn Cipher>> {
            self.0.resolve_cipher(choice)
        }
    }

    #[test]
    fn device_custom_resolver_interoperates_with_host_responder() {
        let device_private = [0x11; 32];
        let host_private = [0x22; 32];
        let mut device = snow::Builder::with_resolver(
            PATTERN.parse().unwrap(),
            Box::new(DeviceResolver(DefaultResolver)),
        )
        .prologue(&PRELUDE)
        .unwrap()
        .local_private_key(&device_private)
        .unwrap()
        .fixed_ephemeral_key_for_testing_only(&[0x33; 32])
        .build_initiator()
        .unwrap();
        let mut host = snow::Builder::new(PATTERN.parse().unwrap())
            .prologue(&PRELUDE)
            .unwrap()
            .local_private_key(&host_private)
            .unwrap()
            .fixed_ephemeral_key_for_testing_only(&[0x44; 32])
            .build_responder()
            .unwrap();

        let mut first = [0; 1_024];
        let first_length = device.write_message(&[], &mut first).unwrap();
        let mut scratch = [0; 1_024];
        host.read_message(&first[..first_length], &mut scratch)
            .unwrap();
        let mut second = [0; 1_024];
        let second_length = host.write_message(&[], &mut second).unwrap();
        device
            .read_message(&second[..second_length], &mut scratch)
            .unwrap();
        let mut third = [0; 1_024];
        let third_length = device.write_message(&[], &mut third).unwrap();
        host.read_message(&third[..third_length], &mut scratch)
            .unwrap();

        assert_eq!(device.get_handshake_hash(), host.get_handshake_hash());
        assert_eq!(
            authentication_string(device.get_handshake_hash()).unwrap(),
            authentication_string(host.get_handshake_hash()).unwrap()
        );
        assert_eq!(device.get_remote_static().unwrap().len(), 32);
        assert_eq!(host.get_remote_static().unwrap().len(), 32);

        let mut device = device.into_transport_mode().unwrap();
        let mut host = host.into_transport_mode().unwrap();
        let message = Message::ReadAvailability {
            request_id: 7,
            operation: [0x55; 16],
        };
        let mut plain = [0; 64];
        let encoded = message.encode(&mut plain).unwrap();
        let mut encrypted = [0; 128];
        let encrypted_length = device.write_message(encoded, &mut encrypted).unwrap();
        let mut decoded = [0; 64];
        let decoded_length = host
            .read_message(&encrypted[..encrypted_length], &mut decoded)
            .unwrap();
        assert_eq!(Message::decode(&decoded[..decoded_length]), Ok(message));
    }
}
