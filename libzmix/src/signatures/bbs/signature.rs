use crate::errors::prelude::*;
use super::keys::{PublicKey, SecretKey};
use amcl_wrapper::{
    group_elem_g1::G1,
    group_elem_g2::G2,
    group_elem::{GroupElement, GroupElementVector},
    field_elem::FieldElement,
    extension_field_gt::GT,
    constants::{GROUP_G1_SIZE, MODBYTES}
};

use amcl_wrapper::group_elem_g1::G1Vector;
use amcl_wrapper::field_elem::FieldElementVector;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Signature {
    pub a: G1,
    pub e: FieldElement,
    pub s: FieldElement
}

// https://eprint.iacr.org/2016/663.pdf Section 4.3
impl Signature {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(GROUP_G1_SIZE + MODBYTES * 2);
        out.extend_from_slice(self.a.to_bytes().as_slice());
        out.extend_from_slice(self.e.to_bytes().as_slice());
        out.extend_from_slice(self.s.to_bytes().as_slice());
        out
    }

    pub fn from_bytes(data: &[u8]) -> Result<Signature, BBSError> {
        let expected = GROUP_G1_SIZE + MODBYTES * 2;
        if data.len() != expected {
            return Err(BBSError::from_kind(BBSErrorKind::SignatureIncorrectSize(
                data.len()
            )));
        }
        let mut index = 0;
        let a = G1::from_bytes(&data[0..GROUP_G1_SIZE]).map_err(|_| BBSError::from_kind(BBSErrorKind::SignatureValueIncorrectSize))?;
        index += GROUP_G1_SIZE;
        let e = FieldElement::from_bytes(&data[index..(index+MODBYTES)]).map_err(|_| BBSError::from_kind(BBSErrorKind::SignatureValueIncorrectSize))?;
        index += MODBYTES;
        let s = FieldElement::from_bytes(&data[index..(index+MODBYTES)]).map_err(|_| BBSError::from_kind(BBSErrorKind::SignatureValueIncorrectSize))?;
        Ok(Signature { a, e, s })
    }

    // No committed messages, All messages known to signer.
    pub fn new(messages: &[FieldElement], signkey: &SecretKey, verkey: &PublicKey) -> Result<Self, BBSError> {
        check_verkey_message(messages, verkey)?;
        let e = FieldElement::random();
        let s = FieldElement::random();
        let b = compute_b_const_time(&G1::new(), verkey, messages, &s, 0);
        let mut exp = signkey.clone();
        exp += &e;
        exp.inverse_mut();
        let a = b * exp;
        Ok(Signature { a, e, s })
    }

    // 1 or more messages are captured in a commitment `commitment`. The remaining known messages are in `messages`.
    // This is a blind signature.
    pub fn new_with_committed_messages(commitment: &G1, messages: &[FieldElement], signkey: &SecretKey, verkey: &PublicKey) -> Result<Self, BBSError> {
        if messages.len() >= verkey.message_count() {
            return Err(BBSError::from_kind(BBSErrorKind::SigningErrorMessageCountMismatch(verkey.message_count(), messages.len())));
        }
        let e = FieldElement::random();
        let s = FieldElement::random();
        let b = compute_b_const_time(commitment, verkey, messages, &s, verkey.message_count() - messages.len());
        let mut exp = signkey.clone();
        exp += &e;
        exp.inverse_mut();
        let a = b * exp;
        Ok(Signature { a, e, s })
    }

    // Once signature on committed attributes (blind signature) is received, the signature needs to be unblinded.
    // Takes the blinding used in the commitment.
    pub fn get_unblinded_signature(&self, blinding: &FieldElement) -> Self {
        Signature { a: self.a.clone(), s: self.s.clone() + blinding, e: self.e.clone() }
    }

    // Verify a signature. During proof of knowledge also, this method is used after extending the verkey
    pub fn verify(&self, messages: &[FieldElement], verkey: &PublicKey) -> Result<bool, BBSError> {
        check_verkey_message(messages, verkey)?;
        let b = compute_b_const_time(&G1::new(), verkey, messages, &self.s, 0);
        let a = (&G2::generator() * &self.e) + &verkey.w;
        Ok(GT::ate_2_pairing_cmp(&self.a, &a, &b, &G2::generator()))
    }
}

fn prep_vec_for_b(public_key: &PublicKey, messages: &[FieldElement], blinding_factor: &FieldElement, offset: usize) -> (G1Vector, FieldElementVector) {
    let mut points = G1Vector::with_capacity(messages.len()+2);
    let mut scalars = FieldElementVector::with_capacity(messages.len()+2);
    // XXX: g1 should not be a generator but a setup param
    // prep for g1*h0^blinding_factor*hi^mi.....
    points.push(G1::generator());
    scalars.push(FieldElement::one());
    points.push(public_key.h0.clone());
    scalars.push(blinding_factor.clone());

    for i in 0..messages.len() {
        points.push(public_key.h[offset+i].clone());
        scalars.push(messages[i].clone());
    }
    (points, scalars)
}

pub fn compute_b_const_time(starting_value: &G1, public_key: &PublicKey, messages: &[FieldElement], blinding_factor: &FieldElement, offset: usize) -> G1 {
    let (points, scalars) = prep_vec_for_b(public_key, messages, blinding_factor, offset);
    starting_value + points.multi_scalar_mul_const_time(&scalars).unwrap()
}

pub fn compute_b_var_time(starting_value: &G1, public_key: &PublicKey, messages: &[FieldElement], blinding_factor: &FieldElement, offset: usize) -> G1 {
    let (points, scalars) = prep_vec_for_b(public_key, messages, blinding_factor, offset);
    starting_value + points.multi_scalar_mul_var_time(&scalars).unwrap()
}

fn check_verkey_message(messages: &[FieldElement], verkey: &PublicKey) -> Result<(), BBSError> {
    if messages.len() != verkey.message_count() {
        return Err(BBSError::from_kind(BBSErrorKind::SigningErrorMessageCountMismatch(verkey.message_count(), messages.len())));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::keys::generate;
    use super::super::pok_sig::ProverCommittingG1;

    #[test]
    fn signature_serialization() {
        let sig = Signature { a: G1::random(), e: FieldElement::random(), s: FieldElement::random() };
        let bytes = sig.to_bytes();
        assert_eq!(bytes.len(), GROUP_G1_SIZE + MODBYTES * 2);
        let sig_2 = Signature::from_bytes(bytes.as_slice()).unwrap();
        assert_eq!(sig, sig_2);
    }

    #[test]
    fn gen_signature() {
        let message_count = 5;
        let mut messages = Vec::new();
        for _ in 0..message_count {
            messages.push(FieldElement::random());
        }
        let (verkey, signkey) = generate(message_count);

        let res = Signature::new(messages.as_slice(), &signkey, &verkey);
        assert!(res.is_ok());
        messages = Vec::new();
        let res = Signature::new(messages.as_slice(), &signkey, &verkey);
        assert!(res.is_err());
    }

    #[test]
    fn signature_validation() {
        let message_count = 5;
        let mut messages = Vec::new();
        for _ in 0..message_count {
            messages.push(FieldElement::random());
        }
        let (verkey, signkey) = generate(message_count);

        let sig = Signature::new(messages.as_slice(), &signkey, &verkey).unwrap();
        let res = sig.verify(messages.as_slice(), &verkey);
        assert!(res.is_ok());
        assert!(res.unwrap());

        messages = Vec::new();
        for _ in 0..message_count {
            messages.push(FieldElement::random());
        }
        let res = sig.verify(messages.as_slice(), &verkey);
        assert!(res.is_ok());
        assert!(!res.unwrap());
    }

    #[test]
    fn signature_committed_messages() {
        let message_count = 4;
        let mut messages = Vec::new();
        for _ in 0..message_count {
            messages.push(FieldElement::random());
        }
        let (verkey, signkey) = generate(message_count);

        //User blinds first attribute
        let blinding = FieldElement::random();

        //User creates a random commitment, computes challenges and response. The proof of knowledge consists of a commitment and responses
        //User and signer engage in a proof of knowledge for `commitment`
        let mut commitment = &verkey.h0 * &blinding + &verkey.h[0] * &messages[0];

        let mut committing = ProverCommittingG1::new();
        committing.commit(&verkey.h0, None);
        committing.commit(&verkey.h[0], None);
        let committed = committing.finish();

        let mut hidden_msgs = Vec::new();
        hidden_msgs.push(blinding.clone());
        hidden_msgs.push(messages[0].clone());

        let mut bases = Vec::new();
        bases.push(verkey.h0.clone());
        bases.push(verkey.h[0].clone());

        let challenge_hash = committed.gen_challenge(commitment.to_bytes());
        let proof = committed.gen_proof(&challenge_hash, hidden_msgs.as_slice()).unwrap();

        assert!(proof.verify(bases.as_slice(), &commitment, &challenge_hash).unwrap());
        let sig = Signature::new_with_committed_messages(&commitment, &messages[1..], &signkey, &verkey);
        assert!(sig.is_ok());
        let sig = sig.unwrap();
        //First test should fail since the signature is blinded
        let res = sig.verify(messages.as_slice(), &verkey);
        assert!(res.is_ok());
        assert!(!res.unwrap());

        let sig = sig.get_unblinded_signature(&blinding);
        let res = sig.verify(messages.as_slice(), &verkey);
        assert!(res.is_ok());
        assert!(res.unwrap());
    }
}