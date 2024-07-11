use crate::air::{BaseAirBuilder, MachineAir, Polynomial, SP1AirBuilder, WORD_SIZE};
use crate::bytes::event::ByteRecord;
use crate::bytes::ByteLookupEvent;
use crate::memory::{value_as_limbs, MemoryReadCols, MemoryWriteCols};
use crate::operations::field::field_op::{FieldOpCols, FieldOperation};
use crate::operations::field::params::{FieldParameters, NumWords};
use crate::operations::field::params::{Limbs, NumLimbs};
use crate::operations::IsZeroOperation;
use crate::runtime::{ExecutionRecord, Program, Syscall, SyscallCode};
use crate::runtime::{MemoryReadRecord, MemoryWriteRecord};
use crate::stark::MachineRecord;
use crate::syscall::precompiles::SyscallContext;
use crate::utils::ec::uint::U384Field;
use crate::utils::{
    bytes_to_words_le, limbs_from_access, limbs_from_prev_access, pad_rows, words_to_bytes_le,
    words_to_bytes_le_vec,
};
use amcl::bls381::fp12;
use generic_array::GenericArray;
use itertools::{izip, Itertools};
use num::Zero;
use num::{BigUint, One};
use p3_air::AirBuilder;
use p3_air::{Air, BaseAir};
use p3_field::AbstractField;
use p3_field::PrimeField32;
use p3_matrix::dense::RowMajorMatrix;
use p3_matrix::Matrix;
use serde::{Deserialize, Serialize};
use sp1_derive::AlignedBorrow;
use std::borrow::{Borrow, BorrowMut};
use std::iter::Sum;
use std::marker::PhantomData;
use std::mem::size_of;
use typenum::Unsigned;

/// The number of columns in the FpMulCols.
const NUM_COLS: usize = size_of::<Fp12MulCols<u8>>();

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fp12MulEvent {
    pub lookup_id: usize,
    pub shard: u32,
    pub channel: u32,
    pub clk: u32,
    pub x_ptr: u32,
    pub x: Vec<u32>,
    pub y_ptr: u32,
    pub y: Vec<u32>,
    pub x_memory_records: Vec<MemoryWriteRecord>,
    pub y_memory_records: Vec<MemoryReadRecord>,
}

type WordsFieldElement = <U384Field as NumWords>::WordsFieldElement;
const WORDS_FIELD_ELEMENT: usize = WordsFieldElement::USIZE;
const NUM_FP_MULS: usize = 144;

#[derive(Debug, Clone, AlignedBorrow)]
#[repr(C)]
pub struct SumOfProductsAuxillaryCols<T> {
    pub b10_p_b11: FieldOpCols<T, U384Field>, // b.c1.c0 + b.c1.c1;
    pub b10_m_b11: FieldOpCols<T, U384Field>, // b.c1.c0 - b.c1.c1;
    pub b20_p_b21: FieldOpCols<T, U384Field>, // b.c2.c0 + b.c2.c1;
    pub b20_m_b21: FieldOpCols<T, U384Field>, // b.c2.c0 - b.c2.c1;
}

#[derive(Debug, Clone, AlignedBorrow)]
#[repr(C)]
pub struct SumOfProductsCols<T> {
    pub a1_t_b1: FieldOpCols<T, U384Field>,
    pub a2_t_b2: FieldOpCols<T, U384Field>,
    pub a3_t_b3: FieldOpCols<T, U384Field>,
    pub a4_t_b4: FieldOpCols<T, U384Field>,
    pub a5_t_b5: FieldOpCols<T, U384Field>,
    pub a6_t_b6: FieldOpCols<T, U384Field>,

    pub sum1: FieldOpCols<T, U384Field>,
    pub sum2: FieldOpCols<T, U384Field>,
    pub sum3: FieldOpCols<T, U384Field>,
    pub sum4: FieldOpCols<T, U384Field>,
    pub sum5: FieldOpCols<T, U384Field>,
}

#[derive(Debug, Clone, AlignedBorrow)]
#[repr(C)]
pub struct Fp6MulCols<T> {
    pub aux: SumOfProductsAuxillaryCols<T>,
    // [a.c0.c0, -a.c0.c1, a.c1.c0, -a.c1.c1, a.c2.c0, -a.c2.c1]
    // [b.c0.c0, b.c0.c1, b20_m_b21, b20_p_b21, b10_m_b11, b10_p_b11]
    pub c00: SumOfProductsCols<T>,

    // [a.c0.c0, a.c0.c1, a.c1.c0, a.c1.c1, a.c2.c0, a.c2.c1],
    // [b.c0.c1, b.c0.c0, b20_p_b21, b20_m_b21, b10_p_b11, b10_m_b11],
    pub c01: SumOfProductsCols<T>,

    // [a.c0.c0, -a.c0.c1, a.c1.c0, -a.c1.c1, a.c2.c0, -a.c2.c1],
    // [b.c1.c0, b.c1.c1, b.c0.c0, b.c0.c1, b20_m_b21, b20_p_b21],
    pub c10: SumOfProductsCols<T>,

    // [a.c0.c0, a.c0.c1, a.c1.c0, a.c1.c1, a.c2.c0, a.c2.c1],
    // [b.c1.c1, b.c1.c0, b.c0.c1, b.c0.c0, b20_p_b21, b20_m_b21],
    pub c11: SumOfProductsCols<T>,

    // [a.c0.c0, -a.c0.c1, a.c1.c0, -a.c1.c1, a.c2.c0, -a.c2.c1],
    // [b.c2.c0, b.c2.c1, b.c1.c0, b.c1.c1, b.c0.c0, b.c0.c1],
    pub c20: SumOfProductsCols<T>,

    // [a.c0.c0, a.c0.c1, a.c1.c0, a.c1.c1, a.c2.c0, a.c2.c1],
    // [b.c2.c1, b.c2.c0, b.c1.c1, b.c1.c0, b.c0.c1, b.c0.c0],
    pub c21: SumOfProductsCols<T>,
}

#[derive(Debug, Clone, AlignedBorrow)]
#[repr(C)]
pub struct Fp6AddCols<T> {
    pub a00_p_b00: FieldOpCols<T, U384Field>, // a.c0.c0 + b.c0.c0
    pub a01_p_b01: FieldOpCols<T, U384Field>, // a.c0.c1 + b.c0.c1
    pub a10_p_b10: FieldOpCols<T, U384Field>, // a.c1.c0 + b.c1.c0
    pub a11_p_b11: FieldOpCols<T, U384Field>, // a.c1.c1 + b.c1.c1
    pub a20_p_b20: FieldOpCols<T, U384Field>, // a.c2.c0 + b.c2.c0
    pub a21_p_b21: FieldOpCols<T, U384Field>, // a.c2.c1 + b.c2.c1
}

#[derive(Debug, Clone, AlignedBorrow)]
#[repr(C)]
pub struct Fp6MulByNonResidueCols<T> {
    pub c00: FieldOpCols<T, U384Field>, // a.c2.c0 - a.c2.c1
    pub c01: FieldOpCols<T, U384Field>, // a.c2.c0 + a.c2.c1

    pub c10: FieldOpCols<T, U384Field>, // a.c0.c0
    pub c11: FieldOpCols<T, U384Field>, // a.c0.c1

    pub c20: FieldOpCols<T, U384Field>, // a.c1.c0
    pub c21: FieldOpCols<T, U384Field>, // a.c1.c1
}

#[derive(Debug, Clone, AlignedBorrow)]
#[repr(C)]
pub struct AuxFp12MulCols<T> {
    pub aa: Fp6MulCols<T>,             // self.c0 * other.c0;
    pub bb: Fp6MulCols<T>,             // self.c1 * other.c1;
    pub o: Fp6AddCols<T>,              // other.c0 + other.c1;
    pub y1: Fp6AddCols<T>,             // a.c1 + a.c0
    pub y2: Fp6MulCols<T>,             // (a.c1 + a.c0) * a.o
    pub y3: Fp6AddCols<T>,             // (a.c1 + a.c0) * o  - aa
    pub y: Fp6AddCols<T>,              // (a.c1 + a.c0) * o  - aa - bb
    pub x1: Fp6MulByNonResidueCols<T>, // bb * non_residue
    pub x: Fp6AddCols<T>,              // bb * non_residue + aa
}

/// A set of columns for the FpMul operation.
#[derive(Debug, Clone, AlignedBorrow)]
#[repr(C)]
pub struct Fp12MulCols<T> {
    pub is_real: T,
    pub shard: T,
    pub channel: T,
    pub clk: T,
    pub nonce: T,
    pub x_ptr: T,
    pub y_ptr: T,
    pub x_memory: GenericArray<MemoryWriteCols<T>, WordsFieldElement>,
    pub y_memory: GenericArray<MemoryReadCols<T>, WordsFieldElement>,
    pub output: AuxFp12MulCols<T>,
}

#[derive(Default)]
pub struct Fp12MulChip<P> {
    _marker: PhantomData<P>,
}

impl<F: PrimeField32, P: FieldParameters> MachineAir<F> for Fp12MulChip<P> {
    type Record = ExecutionRecord;

    type Program = Program;

    fn name(&self) -> String {
        "Fp12Mul".to_string()
    }

    fn generate_trace(&self, input: &Self::Record, output: &mut Self::Record) -> RowMajorMatrix<F> {
        let rows_and_records = input
            .fp12_mul_events
            .chunks(1)
            .map(|events| {
                let mut records = ExecutionRecord::default();
                let mut new_byte_lookup_events = Vec::new();

                let rows = events
                    .iter()
                    .map(|event| {
                        let mut row: [F; NUM_COLS] = [F::zero(); NUM_COLS];
                        let cols: &mut Fp12MulCols<F> = row.as_mut_slice().borrow_mut();
                        // let x = [0..BigUint::from_bytes_le(bytes_to_words_le::<48>(&event.x[0..48]));
                        let x = (0..12)
                            .map(|i| {
                                BigUint::from_bytes_le(&words_to_bytes_le::<48>(
                                    &event.x[i * 48..(i + 1) * 48],
                                ))
                            })
                            .collect_vec();
                        let x: [BigUint; 12] = x.try_into().unwrap();

                        let y = (0..12)
                            .map(|i| {
                                BigUint::from_bytes_le(&words_to_bytes_le::<48>(
                                    &event.y[i * 48..(i + 1) * 48],
                                ))
                            })
                            .collect_vec();
                        let y: [BigUint; 12] = y.try_into().unwrap();

                        let modulus = BigUint::from_bytes_le(P::MODULUS);

                        // Assign basic values to the columns.
                        cols.is_real = F::one();
                        cols.shard = F::from_canonical_u32(event.shard);
                        cols.channel = F::from_canonical_u32(event.channel);
                        cols.clk = F::from_canonical_u32(event.clk);
                        cols.x_ptr = F::from_canonical_u32(event.x_ptr);
                        cols.y_ptr = F::from_canonical_u32(event.y_ptr);

                        // Populate memory columns.
                        for i in 0..WORDS_FIELD_ELEMENT {
                            cols.x_memory[i].populate(
                                event.channel,
                                event.x_memory_records[i],
                                &mut new_byte_lookup_events,
                            );
                            cols.y_memory[i].populate(
                                event.channel,
                                event.y_memory_records[i],
                                &mut new_byte_lookup_events,
                            );
                        }

                        // // Populate the output columns.
                        // let a000 = &x[0];
                        // let a001 = &x[1];
                        // let a010 = &x[2];
                        // let a011 = &x[3];
                        // let a020 = &x[4];
                        // let a021 = &x[5];
                        // let a100 = &x[6];
                        // let a101 = &x[7];
                        // let a110 = &x[8];
                        // let a111 = &x[9];
                        // let a120 = &x[10];
                        // let a121 = &x[11];

                        // let b000 = &y[0];
                        // let b001 = &y[1];
                        // let b010 = &y[2];
                        // let b011 = &y[3];
                        // let b020 = &y[4];
                        // let b021 = &y[5];
                        // let b100 = &y[6];
                        // let b101 = &y[7];
                        // let b110 = &y[8];
                        // let b111 = &y[9];
                        // let b120 = &y[10];
                        // let b121 = &y[11];

                        let mut constraints: Fp12MulChipConstraints<F> =
                            Fp12MulChipConstraints::new(
                                event.shard,
                                event.channel,
                                new_byte_lookup_events.clone(),
                                modulus,
                            );

                        row
                    })
                    .collect_vec();
            })
            .collect::<Vec<_>>();
        todo!()
    }

    fn included(&self, shard: &Self::Record) -> bool {
        todo!()
    }

    fn generate_dependencies(&self, input: &Self::Record, output: &mut Self::Record) {
        self.generate_trace(input, output);
    }

    fn preprocessed_width(&self) -> usize {
        0
    }

    fn generate_preprocessed_trace(&self, _program: &Self::Program) -> Option<RowMajorMatrix<F>> {
        None
    }
}

#[repr(C)]
struct Fp12MulChipConstraints<F> {
    shard: u32,
    channel: u32,
    new_byte_lookup_events: Vec<ByteLookupEvent>,
    modulus: BigUint,
    _marker: PhantomData<F>,
}

impl<F: PrimeField32> Fp12MulChipConstraints<F> {
    fn new(
        shard: u32,
        channel: u32,
        new_byte_lookup_events: Vec<ByteLookupEvent>,
        modulus: BigUint,
    ) -> Self {
        Self {
            shard,
            channel,
            new_byte_lookup_events,
            modulus,
            _marker: PhantomData,
        }
    }

    fn populate_with_modulus(
        &mut self,
        dest: &mut FieldOpCols<F, U384Field>,
        a: &BigUint,
        b: &BigUint,
        op: FieldOperation,
    ) {
        dest.populate_with_modulus(
            &mut self.new_byte_lookup_events,
            self.shard,
            self.channel,
            a,
            b,
            &self.modulus,
            op,
        );
    }
    fn mul(&mut self, dest: &mut FieldOpCols<F, U384Field>, a: &BigUint, b: &BigUint) -> BigUint {
        self.populate_with_modulus(dest, a, b, FieldOperation::Mul);
        (a * b) % &self.modulus
    }
    fn add(&mut self, dest: &mut FieldOpCols<F, U384Field>, a: &BigUint, b: &BigUint) -> BigUint {
        self.populate_with_modulus(dest, a, b, FieldOperation::Mul);
        (a + b) % &self.modulus
    }
    fn sub(&mut self, dest: &mut FieldOpCols<F, U384Field>, a: &BigUint, b: &BigUint) -> BigUint {
        self.populate_with_modulus(dest, a, b, FieldOperation::Mul);
        (a - b) % &self.modulus
    }
    fn sum_of_products_aux(
        &mut self,
        dest: &mut SumOfProductsAuxillaryCols<F>,
        b: [&BigUint; 6],
    ) -> [BigUint; 4] {
        let b00 = b[0];
        let b01 = b[1];
        let b10 = b[2];
        let b11 = b[3];
        let b20 = b[4];
        let b21 = b[5];

        let b10_p_b11 = self.add(&mut dest.b10_p_b11, b10, b11);
        let b10_m_b11 = self.sub(&mut dest.b10_m_b11, b10, b11);
        let b20_p_b21 = self.add(&mut dest.b20_p_b21, b20, b21);
        let b20_m_b21 = self.sub(&mut dest.b20_m_b21, b20, b21);

        [b10_p_b11, b10_m_b11, b20_p_b21, b20_m_b21]
    }

    fn sum_of_products(
        &mut self,
        dest: &mut SumOfProductsCols<F>,
        a: [(i8, &BigUint); 6],
        b: [(i8, &BigUint); 6],
    ) -> BigUint {
        let a00 = a[0].1;
        let a01 = a[1].1;
        let a10 = a[2].1;
        let a11 = a[3].1;
        let a20 = a[4].1;
        let a21 = a[5].1;

        let b00 = b[0].1;
        let b01 = b[1].1;
        let b10 = b[2].1;
        let b11 = b[3].1;
        let b20 = b[4].1;
        let b21 = b[5].1;

        let a1_t_b1 = &self.mul(&mut dest.a1_t_b1, a00, b00);
        let a2_t_b2 = &self.mul(&mut dest.a2_t_b2, a01, b01);
        let a3_t_b3 = &self.mul(&mut dest.a3_t_b3, a10, b10);
        let a4_t_b4 = &self.mul(&mut dest.a4_t_b4, a11, b11);
        let a5_t_b5 = &self.mul(&mut dest.a5_t_b5, a20, b20);
        let a6_t_b6 = &self.mul(&mut dest.a6_t_b6, a21, b21);

        let products = [a1_t_b1, a2_t_b2, a3_t_b3, a4_t_b4, a5_t_b5, a6_t_b6];
        let dests = [
            &mut dest.sum1,
            &mut dest.sum2,
            &mut dest.sum3,
            &mut dest.sum4,
            &mut dest.sum5,
        ];

        // Get negative coefficients in the sum of products.
        let is_sub = a
            .iter()
            .zip(b.iter())
            .map(|(a, b)| a.0 != b.0)
            .collect_vec();

        let mut sum = a1_t_b1.clone();

        for (is_neg, dest, cur) in izip!(is_sub, dests, products).skip(1) {
            let _sum = &sum.clone();
            if is_neg {
                sum = sum + &self.sub(dest, _sum, cur);
            } else {
                sum = sum + &self.add(dest, _sum, cur);
            }
        }

        sum
    }
    fn fp6_mul(
        &mut self,
        dest: &mut Fp6MulCols<F>,
        a: &[BigUint; 6],
        b: &[BigUint; 6],
    ) -> [BigUint; 6] {
        let a00 = &a[0];
        let a01 = &a[1];
        let a10 = &a[2];
        let a11 = &a[3];
        let a20 = &a[4];
        let a21 = &a[5];

        let b00 = &b[0];
        let b01 = &b[1];
        let b10 = &b[2];
        let b11 = &b[3];
        let b20 = &b[4];
        let b21 = &b[5];

        let [b10_p_b11, b10_m_b11, b20_p_b21, b20_m_b21] =
            self.sum_of_products_aux(&mut dest.aux, [b00, b01, b10, b11, b20, b21]);
        let c00 = self.sum_of_products(
            &mut dest.c00,
            [(1, a00), (1, a01), (1, a10), (1, a11), (1, a20), (1, a21)],
            [
                (1, b00),
                (-1, b01),
                (1, &b20_m_b21),
                (-1, &b20_p_b21),
                (1, &b10_m_b11),
                (-1, &b10_p_b11),
            ],
        );

        let c01 = self.sum_of_products(
            &mut dest.c01,
            [(1, a00), (1, a01), (1, a10), (1, a11), (1, a20), (1, a21)],
            [
                (1, b01),
                (1, b00),
                (1, &b20_p_b21),
                (1, &b20_m_b21),
                (1, &b10_p_b11),
                (1, &b10_m_b11),
            ],
        );

        let c10 = self.sum_of_products(
            &mut dest.c10,
            [
                (1, a00),
                (-1, a01),
                (1, a10),
                (-1, a11),
                (1, a20),
                (-1, a21),
            ],
            [
                (1, b10),
                (1, b11),
                (1, b00),
                (1, b01),
                (1, &b20_m_b21),
                (1, &b20_p_b21),
            ],
        );

        let c11 = self.sum_of_products(
            &mut dest.c11,
            [(1, a00), (1, a01), (1, a10), (1, a11), (1, a20), (1, a21)],
            [
                (1, b11),
                (1, b10),
                (1, b01),
                (1, b00),
                (1, &b20_p_b21),
                (1, &b20_m_b21),
            ],
        );

        let c20 = self.sum_of_products(
            &mut dest.c20,
            [
                (1, a00),
                (-1, a01),
                (1, a10),
                (-1, a11),
                (1, a20),
                (-1, a21),
            ],
            [(1, b20), (1, b21), (1, b10), (1, b11), (1, b00), (1, b01)],
        );

        let c21 = self.sum_of_products(
            &mut dest.c21,
            [(1, a00), (1, a01), (1, a10), (1, a11), (1, a20), (1, a21)],
            [(1, b21), (1, b20), (1, b11), (1, b10), (1, b01), (1, b00)],
        );

        [c00, c01, c10, c11, c20, c21]
    }
    fn fp6_add(
        &mut self,
        dest: &mut Fp6AddCols<F>,
        a: &[BigUint; 6],
        b: &[BigUint; 6],
    ) -> [BigUint; 6] {
        let a00 = &a[0];
        let a01 = &a[1];
        let a10 = &a[2];
        let a11 = &a[3];
        let a20 = &a[4];
        let a21 = &a[5];

        let b00 = &b[0];
        let b01 = &b[1];
        let b10 = &b[2];
        let b11 = &b[3];
        let b20 = &b[4];
        let b21 = &b[5];

        let a00_p_b00 = self.add(&mut dest.a00_p_b00, a00, b00);
        let a01_p_b01 = self.add(&mut dest.a01_p_b01, a01, b01);
        let a10_p_b10 = self.add(&mut dest.a10_p_b10, a10, b10);
        let a11_p_b11 = self.add(&mut dest.a11_p_b11, a11, b11);
        let a20_p_b20 = self.add(&mut dest.a20_p_b20, a20, b20);
        let a21_p_b21 = self.add(&mut dest.a21_p_b21, a21, b21);

        [
            a00_p_b00, a01_p_b01, a10_p_b10, a11_p_b11, a20_p_b20, a21_p_b21,
        ]
    }

    fn fp6_sub(
        &mut self,
        dest: &mut Fp6AddCols<F>,
        a: &[BigUint; 6],
        b: &[BigUint; 6],
    ) -> [BigUint; 6] {
        let a00 = &a[0];
        let a01 = &a[1];
        let a10 = &a[2];
        let a11 = &a[3];
        let a20 = &a[4];
        let a21 = &a[5];

        let b00 = &b[0];
        let b01 = &b[1];
        let b10 = &b[2];
        let b11 = &b[3];
        let b20 = &b[4];
        let b21 = &b[5];

        let a00_m_b00 = self.sub(&mut dest.a00_p_b00, a00, b00);
        let a01_m_b01 = self.sub(&mut dest.a01_p_b01, a01, b01);
        let a10_m_b10 = self.sub(&mut dest.a10_p_b10, a10, b10);
        let a11_m_b11 = self.sub(&mut dest.a11_p_b11, a11, b11);
        let a20_m_b20 = self.sub(&mut dest.a20_p_b20, a20, b20);
        let a21_m_b21 = self.sub(&mut dest.a21_p_b21, a21, b21);

        [
            a00_m_b00, a01_m_b01, a10_m_b10, a11_m_b11, a20_m_b20, a21_m_b21,
        ]
    }
    fn fp6_mul_by_non_residue(
        &mut self,
        dest: &mut Fp6MulByNonResidueCols<F>,
        a: &[BigUint; 6],
    ) -> [BigUint; 6] {
        let a00 = &a[0];
        let a01 = &a[1];
        let a10 = &a[2];
        let a11 = &a[3];
        let a20 = &a[4];
        let a21 = &a[5];

        let c00 = self.sub(&mut dest.c00, &a20, &a21);
        let c01 = self.add(&mut dest.c01, &a20, &a21);

        let c10 = a00;
        let c11 = a01;

        let c20 = a10;
        let c21 = a11;

        [c00, c01, c10.clone(), c11.clone(), c20.clone(), c21.clone()]
    }
    fn fp12_mul(
        &mut self,
        dest: &mut AuxFp12MulCols<F>,
        a: [BigUint; 12],
        b: [BigUint; 12],
    ) -> [BigUint; 12] {
        let ac0 = a[0..6]
            .iter()
            .map(|x| x.clone())
            .collect_vec()
            .try_into()
            .unwrap();
        let ac1 = a[6..12]
            .iter()
            .map(|x| x.clone())
            .collect_vec()
            .try_into()
            .unwrap();
        let bc0 = b[0..6]
            .iter()
            .map(|x| x.clone())
            .collect_vec()
            .try_into()
            .unwrap();
        let bc1 = b[6..12]
            .iter()
            .map(|x| x.clone())
            .collect_vec()
            .try_into()
            .unwrap();

        let aa = self.fp6_mul(&mut dest.aa, &ac0, &bc0);
        let bb = self.fp6_mul(&mut dest.bb, &ac1, &bc1);

        let o = self.fp6_add(&mut dest.o, &bc0, &bc0);
        let y1 = self.fp6_add(&mut dest.y1, &ac1, &ac0);
        let y2 = self.fp6_mul(&mut dest.y2, &y1, &o);
        let y3 = self.fp6_sub(&mut dest.y3, &y2, &aa);
        let y = self.fp6_sub(&mut dest.y, &y3, &bb);
        let x1 = self.fp6_mul_by_non_residue(&mut dest.x1, &bb);
        let x = self.fp6_add(&mut dest.x, &x1, &aa);

        x.iter()
            .chain(y.iter())
            .cloned()
            .collect_vec()
            .try_into()
            .unwrap()
    }
}
