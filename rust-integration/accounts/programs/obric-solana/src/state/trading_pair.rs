use crate::{consts::MILLION, errors::ObricError};
use anchor_lang::prelude::*;
use num::{integer::Roots, pow};

#[account]
#[derive(Default, Debug, Copy)]
pub struct SSTradingPair {
    pub is_initialized: bool,

    pub x_price_feed_id: Pubkey,
    pub y_price_feed_id: Pubkey,

    pub reserve_x: Pubkey,
    pub reserve_y: Pubkey,

    pub reference_oracle: Pubkey,
    pub second_reference_oracle: Pubkey,

    pub bump: u8,
    // mints
    pub mint_x: Pubkey,
    pub mint_y: Pubkey,

    pub concentration: u64,
    pub big_k: u128,
    pub target_x: u64,

    pub cumulative_volume: u64,

    pub mult_x: u64,
    pub mult_y: u64,
    pub fee_millionth: u64,

    pub padding1: [u64; 2],

    pub volume_record: [u64; 8],
    pub volume_time_record: [i64; 8],

    pub version: u16,
    pub feed_max_age_x: u8,
    pub feed_max_age_y: u8,

    pub price_decimals: u8,

    pub padding: [u8; 3],

    pub mint_sslp_x: Pubkey,
    pub mint_sslp_y: Pubkey,
    pub secondary_price_x: Pubkey,
    pub secondary_price_y: Pubkey,

    pub whirl_mult: u32,
    pub whirl_divisor: u16,
    pub whirl_enabled: bool,

    pub target_y_based_lock: bool,
    pub reference_target_y: u64,

    pub padding2: [u64; 5],
}

impl SSTradingPair {
    pub fn update_price(
        &mut self,
        price_x: u64,
        price_y: u64,
        x_decimals: u8,
        y_decimals: u8,
    ) -> Result<()> {
        let (x_deci_mult, y_deci_mult) = if x_decimals > y_decimals {
            (1 as u64, pow(10, usize::from(x_decimals - y_decimals)))
        } else if y_decimals > x_decimals {
            (pow(10, usize::from(y_decimals - x_decimals)), 1 as u64)
        } else {
            (1 as u64, 1 as u64)
        };

        self.mult_x = price_x
            .checked_mul(x_deci_mult)
            .ok_or(ObricError::NumOverflowing)?;
        self.mult_y = price_y
            .checked_mul(y_deci_mult)
            .ok_or(ObricError::NumOverflowing)?;

        Ok(())
    }
    pub fn get_target_xy(&self, current_x: u64, current_y: u64) -> Result<(u64, u64)> {
        let value_x = (current_x as u128)
            .checked_mul(self.mult_x as u128)
            .ok_or(ObricError::NumOverflowing)?;
        let value_y = (current_y as u128)
            .checked_mul(self.mult_y as u128)
            .ok_or(ObricError::NumOverflowing)?;
        let value_total = value_x
            .checked_add(value_y)
            .ok_or(ObricError::NumOverflowing)?;

        let target_x = self.target_x;
        let target_x_value = (target_x as u128)
            .checked_mul(self.mult_x as u128)
            .ok_or(ObricError::NumOverflowing)?;
        let target_y_value = value_total
            .checked_sub(target_x_value)
            .ok_or(ObricError::NumOverflowing)?;
        let target_y = (target_y_value
            .checked_div(self.mult_y as u128)
            .ok_or(ObricError::NumOverflowing)?) as u64;
        Ok((target_x, target_y))
    }
    /**
    Returns (output_to_user, fee)
     */
    pub fn quote_x_to_y(
        &self,
        input_x: u64,
        current_x: u64,
        current_y: u64,
    ) -> Result<(u64, u64)> {
        if input_x == 0 {
            return Ok((0u64, 0u64));
        }

        let (target_x, _target_y) = self.get_target_xy(current_x, current_y)?;

        // perform lock-checking
        if self.target_y_based_lock {
            let allow_swap = abs_diff(current_x + input_x, target_x)? < abs_diff(current_x, target_x)?;

            if !allow_swap {
                return Ok((0u64, 0u64));
            }
        }

        // 0. get target_x on curve-K
        let big_k = self.big_k;
        //target_x_K = sqrt(big_k / p), where p = mult_x / mult_y
        let target_x_k = (big_k
            .checked_mul(self.mult_y as u128)
            .ok_or(ObricError::NumOverflowing)?
            .checked_div(self.mult_x as u128)
            .ok_or(ObricError::NumOverflowing)?)
        .sqrt();

        // 1. find current (x,y) on curve-K
        let current_x_k = (target_x_k - target_x as u128)
            .checked_add(current_x as u128)
            .ok_or(ObricError::NumOverflowing)?;
        let current_y_k = big_k
            .checked_div(current_x_k)
            .ok_or(ObricError::NumOverflowing)?;

        // 2. find new (x, y) on curve-K
        let new_x_k = current_x_k
            .checked_add(input_x as u128)
            .ok_or(ObricError::NumOverflowing)?;
        let new_y_k = big_k
            .checked_div(new_x_k)
            .ok_or(ObricError::NumOverflowing)?;

        let output_before_fee_y: u64 = (current_y_k
            .checked_sub(new_y_k)
            .ok_or(ObricError::NumOverflowing)?) as u64;
        if output_before_fee_y >= current_y {
            return Ok((0u64, 0u64));
        }
        let fee_y = output_before_fee_y
            .checked_mul(self.fee_millionth)
            .ok_or(ObricError::NumOverflowing)?
            .checked_div(MILLION)
            .ok_or(ObricError::NumOverflowing)?;
        let output_after_fee_y = output_before_fee_y
            .checked_sub(fee_y)
            .ok_or(ObricError::NumOverflowing)?;

        Ok((output_after_fee_y, fee_y))
    }

    /**
    Returns (output_to_user, fee)
     */
    pub fn quote_y_to_x(
        &self,
        input_y: u64,
        current_x: u64,
        current_y: u64,
    ) -> Result<(u64, u64)> {
        if input_y == 0 {
            return Ok((0u64, 0u64));
        }

        let (target_x, target_y) = self.get_target_xy(current_x, current_y)?;

        // perform lock-checking
        if self.target_y_based_lock {
            let allow_swap = abs_diff(current_y + input_y, target_y)? < abs_diff(current_y, target_y)?;

            if !allow_swap {
                return Ok((0u64, 0u64));
            }
        }

        // 0. get target_x on curve-K
        let big_k = self.big_k;
        //target_x_K = sqrt(big_k / p), where p = mult_x / mult_y
        let target_x_k = (big_k
            .checked_mul(self.mult_y as u128)
            .ok_or(ObricError::NumOverflowing)?
            .checked_div(self.mult_x as u128)
            .ok_or(ObricError::NumOverflowing)?)
        .sqrt();

        // 1. find current (x, y) on curve-K
        let current_x_k = (target_x_k - target_x as u128)
            .checked_add(current_x as u128)
            .ok_or(ObricError::NumOverflowing)?;
        let current_y_k = big_k
            .checked_div(current_x_k)
            .ok_or(ObricError::NumOverflowing)?;

        // 2. find new (x, y) on curve-K
        let new_y_k = current_y_k
            .checked_add(input_y as u128)
            .ok_or(ObricError::NumOverflowing)?;
        let new_x_k = big_k
            .checked_div(new_y_k)
            .ok_or(ObricError::NumOverflowing)?;

        let output_before_fee_x: u64 = (current_x_k
            .checked_sub(new_x_k)
            .ok_or(ObricError::NumOverflowing)?) as u64;
        if output_before_fee_x >= current_x {
            return Ok((0u64, 0u64));
        }

        let fee_x = output_before_fee_x
            .checked_mul(self.fee_millionth)
            .ok_or(ObricError::NumOverflowing)?
            .checked_div(MILLION)
            .ok_or(ObricError::NumOverflowing)?;
        let output_after_fee_x = output_before_fee_x
            .checked_sub(fee_x)
            .ok_or(ObricError::NumOverflowing)?;

        Ok((output_after_fee_x, fee_x))
    }
}

pub fn abs_diff(x: u64, y: u64) -> Result<u64> {
    let val = if x > y {
        x.checked_sub(y).ok_or(ObricError::NumOverflowing)?
    } 
    else {
        y.checked_sub(x).ok_or(ObricError::NumOverflowing)?
    };
    Ok(val)
}
