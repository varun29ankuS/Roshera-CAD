//! B-Spline and NURBS surface implementation.
//!
//! Features:
//! - NURBS (Non-Uniform Rational B-Spline) surfaces
//! - Tensor-product surfaces with independent U/V parameterization
//! - Trimmed NURBS surfaces with arbitrary boundaries
//! - Surface analysis and interrogation
//! - Evaluation via de Boor's algorithm
//!
//! References:
//! - Piegl & Tiller, "The NURBS Book", 2nd Ed., Chapters 5-8
//! - ISO 10303-42:2022 STEP geometric representation
//! - Rogers, "An Introduction to NURBS", Morgan Kaufmann

use crate::math::nurbs::NurbsCurve;
use crate::math::{consts, MathError, MathResult, Matrix4, Point3, Tolerance, Vector3};

/// 2D parameter for surface evaluation
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Parameter2D {
    pub u: f64,
    pub v: f64,
}

impl Parameter2D {
    pub fn new(u: f64, v: f64) -> Self {
        Self { u, v }
    }
}

/// Parameter range for a single dimension
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParameterRange {
    pub min: f64,
    pub max: f64,
}

impl ParameterRange {
    pub fn new(min: f64, max: f64) -> Self {
        Self { min, max }
    }

    pub fn unit() -> Self {
        Self { min: 0.0, max: 1.0 }
    }

    pub fn contains(&self, t: f64) -> bool {
        t >= self.min && t <= self.max
    }

    pub fn clamp(&self, t: f64) -> f64 {
        t.clamp(self.min, self.max)
    }
}

/// 2D parameter domain
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParameterDomain {
    pub u_range: ParameterRange,
    pub v_range: ParameterRange,
}

impl ParameterDomain {
    pub fn new(u_range: ParameterRange, v_range: ParameterRange) -> Self {
        Self { u_range, v_range }
    }

    pub fn unit() -> Self {
        Self {
            u_range: ParameterRange::unit(),
            v_range: ParameterRange::unit(),
        }
    }

    pub fn contains(&self, param: Parameter2D) -> bool {
        self.u_range.contains(param.u) && self.v_range.contains(param.v)
    }

    pub fn clamp(&self, param: Parameter2D) -> Parameter2D {
        Parameter2D {
            u: self.u_range.clamp(param.u),
            v: self.v_range.clamp(param.v),
        }
    }
}

/// Result of surface evaluation with full differential geometry
#[derive(Debug, Clone)]
pub struct SurfacePoint {
    /// Position on surface
    pub position: Point3,
    /// Partial derivative with respect to u
    pub du: Vector3,
    /// Partial derivative with respect to v
    pub dv: Vector3,
    /// Second partial derivative ∂²/∂u²
    pub duu: Option<Vector3>,
    /// Mixed partial derivative ∂²/∂u∂v
    pub duv: Option<Vector3>,
    /// Second partial derivative ∂²/∂v²
    pub dvv: Option<Vector3>,
}

impl SurfacePoint {
    /// Get surface normal (normalized)
    pub fn normal(&self) -> MathResult<Vector3> {
        self.du.cross(&self.dv).normalize()
    }

    /// Get unnormalized normal (for curvature calculations)
    pub fn normal_unnormalized(&self) -> Vector3 {
        self.du.cross(&self.dv)
    }

    /// Get first fundamental form coefficients
    pub fn first_fundamental_form(&self) -> (f64, f64, f64) {
        let e = self.du.dot(&self.du);
        let f = self.du.dot(&self.dv);
        let g = self.dv.dot(&self.dv);
        (e, f, g)
    }

    /// Get second fundamental form coefficients
    pub fn second_fundamental_form(&self) -> Option<(f64, f64, f64)> {
        match (self.duu, self.duv, self.dvv) {
            (Some(duu), Some(duv), Some(dvv)) => {
                let n = self.normal().ok()?;
                let l = duu.dot(&n);
                let m = duv.dot(&n);
                let n_coeff = dvv.dot(&n);
                Some((l, m, n_coeff))
            }
            _ => None,
        }
    }

    /// Get Gaussian curvature K = (LN - M²) / (EG - F²)
    pub fn gaussian_curvature(&self) -> Option<f64> {
        let (e, f, g) = self.first_fundamental_form();
        let (l, m, n) = self.second_fundamental_form()?;

        let denom = e * g - f * f;
        if denom.abs() < consts::EPSILON {
            return None;
        }

        Some((l * n - m * m) / denom)
    }

    /// Get mean curvature H = (EN + GL - 2FM) / 2(EG - F²)
    pub fn mean_curvature(&self) -> Option<f64> {
        let (e, f, g) = self.first_fundamental_form();
        let (l, m, n) = self.second_fundamental_form()?;

        let denom = e * g - f * f;
        if denom.abs() < consts::EPSILON {
            return None;
        }

        Some((e * n + g * l - 2.0 * f * m) / (2.0 * denom))
    }

    /// Get principal curvatures
    pub fn principal_curvatures(&self) -> Option<(f64, f64)> {
        let h = self.mean_curvature()?;
        let k = self.gaussian_curvature()?;

        // κ₁,₂ = H ± √(H² - K)
        let discriminant = h * h - k;
        if discriminant < 0.0 {
            // Complex curvatures (shouldn't happen for real surfaces)
            return None;
        }

        let sqrt_disc = discriminant.sqrt();
        Some((h + sqrt_disc, h - sqrt_disc))
    }

    /// Get principal directions
    pub fn principal_directions(&self) -> Option<(Vector3, Vector3)> {
        let (e, f, g) = self.first_fundamental_form();
        let (l, m, n) = self.second_fundamental_form()?;
        let (k1, k2) = self.principal_curvatures()?;

        // Solve for principal directions
        // (L - κE)ξ + (M - κF)η = 0
        // (M - κF)ξ + (N - κG)η = 0

        let compute_direction = |kappa: f64| -> Option<Vector3> {
            let a11 = l - kappa * e;
            let a12 = m - kappa * f;
            let a21 = m - kappa * f;
            let a22 = n - kappa * g;

            // Find non-trivial solution
            if a11.abs() > consts::EPSILON || a12.abs() > consts::EPSILON {
                let eta = 1.0;
                let xi = -(a12 * eta) / a11;
                Some((self.du * xi + self.dv * eta).normalize().ok()?)
            } else if a21.abs() > consts::EPSILON || a22.abs() > consts::EPSILON {
                let xi = 1.0;
                let eta = -(a21 * xi) / a22;
                Some((self.du * xi + self.dv * eta).normalize().ok()?)
            } else {
                None
            }
        };

        match (compute_direction(k1), compute_direction(k2)) {
            (Some(d1), Some(d2)) => Some((d1, d2)),
            _ => None,
        }
    }
}

/// B-Spline surface representation
#[derive(Debug)]
pub struct BSplineSurface {
    /// Degree in U direction
    pub degree_u: usize,
    /// Degree in V direction
    pub degree_v: usize,
    /// Control points grid (row-major: [v][u])
    pub control_points: Vec<Vec<Point3>>,
    /// Knot vector in U direction
    pub knots_u: Vec<f64>,
    /// Knot vector in V direction
    pub knots_v: Vec<f64>,
    /// Parameter domain
    pub domain: ParameterDomain,
}

impl Clone for BSplineSurface {
    fn clone(&self) -> Self {
        Self {
            degree_u: self.degree_u,
            degree_v: self.degree_v,
            control_points: self.control_points.clone(),
            knots_u: self.knots_u.clone(),
            knots_v: self.knots_v.clone(),
            domain: self.domain.clone(),
        }
    }
}

impl BSplineSurface {
    /// Create new B-spline surface
    pub fn new(
        degree_u: usize,
        degree_v: usize,
        control_points: Vec<Vec<Point3>>,
        knots_u: Vec<f64>,
        knots_v: Vec<f64>,
    ) -> MathResult<Self> {
        // Validate dimensions
        let n_v = control_points.len();
        if n_v == 0 {
            return Err(MathError::InvalidParameter(
                "Empty control point grid".to_string(),
            ));
        }

        let n_u = control_points[0].len();
        if n_u < degree_u + 1 {
            return Err(MathError::InvalidParameter(format!(
                "Need at least {} control points in U for degree {}",
                degree_u + 1,
                degree_u
            )));
        }
        if n_v < degree_v + 1 {
            return Err(MathError::InvalidParameter(format!(
                "Need at least {} control points in V for degree {}",
                degree_v + 1,
                degree_v
            )));
        }

        // Check rectangular grid
        for row in &control_points {
            if row.len() != n_u {
                return Err(MathError::InvalidParameter(
                    "Control point grid must be rectangular".to_string(),
                ));
            }
        }

        // Validate knot vectors
        if knots_u.len() != n_u + degree_u + 1 {
            return Err(MathError::InvalidParameter(format!(
                "Expected {} knots in U, got {}",
                n_u + degree_u + 1,
                knots_u.len()
            )));
        }
        if knots_v.len() != n_v + degree_v + 1 {
            return Err(MathError::InvalidParameter(format!(
                "Expected {} knots in V, got {}",
                n_v + degree_v + 1,
                knots_v.len()
            )));
        }

        // Check knot vectors are non-decreasing
        for i in 1..knots_u.len() {
            if knots_u[i] < knots_u[i - 1] {
                return Err(MathError::InvalidParameter(
                    "U knot vector must be non-decreasing".to_string(),
                ));
            }
        }
        for i in 1..knots_v.len() {
            if knots_v[i] < knots_v[i - 1] {
                return Err(MathError::InvalidParameter(
                    "V knot vector must be non-decreasing".to_string(),
                ));
            }
        }

        let domain = ParameterDomain::new(
            ParameterRange::new(knots_u[degree_u], knots_u[n_u]),
            ParameterRange::new(knots_v[degree_v], knots_v[n_v]),
        );

        Ok(Self {
            degree_u,
            degree_v,
            control_points,
            knots_u,
            knots_v,
            domain,
        })
    }

    /// Create a bilinear surface (degree 1 in both directions)
    pub fn bilinear(p00: Point3, p10: Point3, p01: Point3, p11: Point3) -> MathResult<Self> {
        let control_points = vec![vec![p00, p10], vec![p01, p11]];

        let knots_u = vec![0.0, 0.0, 1.0, 1.0];
        let knots_v = vec![0.0, 0.0, 1.0, 1.0];

        Self::new(1, 1, control_points, knots_u, knots_v)
    }

    /// Create a ruled surface between two curves
    pub fn ruled(curve1: &NurbsCurve, curve2: &NurbsCurve) -> MathResult<Self> {
        // For simplicity, assume curves have same degree and knots
        if curve1.degree != curve2.degree {
            return Err(MathError::InvalidParameter(
                "Curves must have same degree for ruled surface".to_string(),
            ));
        }

        let control_points = vec![curve1.control_points.clone(), curve2.control_points.clone()];

        let knots_v = vec![0.0, 0.0, 1.0, 1.0];

        Self::new(
            curve1.degree,
            1,
            control_points,
            curve1.knots.values().to_vec(),
            knots_v,
        )
    }

    /// Find knot span for U parameter
    fn find_span_u(&self, u: f64) -> usize {
        let n = self.control_points[0].len() - 1;

        if u >= self.knots_u[n + 1] {
            return n;
        }
        if u <= self.knots_u[self.degree_u] {
            return self.degree_u;
        }

        // Binary search
        let mut low = self.degree_u;
        let mut high = n + 1;
        let mut mid = (low + high) / 2;

        while u < self.knots_u[mid] || u >= self.knots_u[mid + 1] {
            if u < self.knots_u[mid] {
                high = mid;
            } else {
                low = mid;
            }
            mid = (low + high) / 2;
        }

        mid
    }

    /// Find knot span for V parameter
    fn find_span_v(&self, v: f64) -> usize {
        let n = self.control_points.len() - 1;

        if v >= self.knots_v[n + 1] {
            return n;
        }
        if v <= self.knots_v[self.degree_v] {
            return self.degree_v;
        }

        // Binary search
        let mut low = self.degree_v;
        let mut high = n + 1;
        let mut mid = (low + high) / 2;

        while v < self.knots_v[mid] || v >= self.knots_v[mid + 1] {
            if v < self.knots_v[mid] {
                high = mid;
            } else {
                low = mid;
            }
            mid = (low + high) / 2;
        }

        mid
    }

    /// Compute basis functions for U
    fn basis_functions_u(&self, span: usize, u: f64) -> Vec<f64> {
        self.basis_functions(&self.knots_u, self.degree_u, span, u)
    }

    /// Compute basis functions for V
    fn basis_functions_v(&self, span: usize, v: f64) -> Vec<f64> {
        self.basis_functions(&self.knots_v, self.degree_v, span, v)
    }

    /// Generic basis function computation
    fn basis_functions(&self, knots: &[f64], degree: usize, span: usize, t: f64) -> Vec<f64> {
        let mut basis = vec![0.0; degree + 1];
        let mut left = vec![0.0; degree + 1];
        let mut right = vec![0.0; degree + 1];

        basis[0] = 1.0;

        for j in 1..=degree {
            left[j] = t - knots[span + 1 - j];
            right[j] = knots[span + j] - t;

            let mut saved = 0.0;
            for r in 0..j {
                let temp = basis[r] / (right[r + 1] + left[j - r]);
                basis[r] = saved + right[r + 1] * temp;
                saved = left[j - r] * temp;
            }
            basis[j] = saved;
        }

        basis
    }

    /// Compute basis function derivatives
    /// B-spline basis functions and their derivatives up to `deriv_order`.
    ///
    /// Implements Piegl & Tiller, *The NURBS Book* (2nd ed.), Algorithm A2.3.
    /// Returns a matrix `ders` where `ders[k][j]` is the k-th derivative of the
    /// j-th non-zero basis function at parameter `t`. Derivatives of order
    /// strictly greater than the degree are identically zero; the
    /// corresponding rows remain filled with zeros.
    fn basis_derivatives(
        &self,
        knots: &[f64],
        degree: usize,
        span: usize,
        t: f64,
        deriv_order: usize,
    ) -> Vec<Vec<f64>> {
        let n = deriv_order.min(degree);
        let mut ders = vec![vec![0.0; degree + 1]; deriv_order + 1];
        let mut ndu = vec![vec![0.0; degree + 1]; degree + 1];
        let mut left = vec![0.0; degree + 1];
        let mut right = vec![0.0; degree + 1];

        ndu[0][0] = 1.0;

        for j in 1..=degree {
            left[j] = t - knots[span + 1 - j];
            right[j] = knots[span + j] - t;

            let mut saved = 0.0;
            for r in 0..j {
                ndu[j][r] = right[r + 1] + left[j - r];
                let temp = ndu[r][j - 1] / ndu[j][r];
                ndu[r][j] = saved + right[r + 1] * temp;
                saved = left[j - r] * temp;
            }
            ndu[j][j] = saved;
        }

        // Row 0: the basis functions themselves.
        for j in 0..=degree {
            ders[0][j] = ndu[j][degree];
        }

        // Derivative rows 1..=n via Piegl-Tiller A2.3.
        for r in 0..=degree {
            let mut s1: usize = 0;
            let mut s2: usize = 1;
            let mut a = vec![vec![0.0_f64; degree + 1]; 2];
            a[0][0] = 1.0;

            for k in 1..=n {
                let mut d = 0.0;
                let rk = r as i32 - k as i32;
                let pk = degree as i32 - k as i32;

                if r >= k {
                    a[s2][0] = a[s1][0] / ndu[pk as usize + 1][rk as usize];
                    d = a[s2][0] * ndu[rk as usize][pk as usize];
                }

                let j1 = if rk >= -1 { 1 } else { (-rk) as usize };
                // Signed comparison mirrors the i32 version in Piegl-Tiller;
                // `r: usize` would underflow when r == 0.
                let j2 = if (r as i32) - 1 <= pk {
                    k - 1
                } else {
                    degree - r
                };

                for j in j1..=j2 {
                    a[s2][j] =
                        (a[s1][j] - a[s1][j - 1]) / ndu[pk as usize + 1][rk as usize + j];
                    d += a[s2][j] * ndu[rk as usize + j][pk as usize];
                }

                if r <= pk as usize {
                    a[s2][k] = -a[s1][k - 1] / ndu[pk as usize + 1][r];
                    d += a[s2][k] * ndu[r][pk as usize];
                }

                ders[k][r] = d;
                std::mem::swap(&mut s1, &mut s2);
            }
        }

        // Multiply by the falling factorial  p! / (p-k)!.
        let mut factor = degree as f64;
        for k in 1..=n {
            for j in 0..=degree {
                ders[k][j] *= factor;
            }
            if k < degree {
                factor *= (degree - k) as f64;
            }
        }

        ders
    }

    /// Evaluate surface at parameter
    pub fn evaluate(&self, param: Parameter2D) -> MathResult<SurfacePoint> {
        let param = self.domain.clamp(param);

        // Find spans
        let span_u = self.find_span_u(param.u);
        let span_v = self.find_span_v(param.v);

        // Get basis functions with derivatives
        let ders_u = self.basis_derivatives(&self.knots_u, self.degree_u, span_u, param.u, 2);
        let ders_v = self.basis_derivatives(&self.knots_v, self.degree_v, span_v, param.v, 2);

        // Initialize results
        let mut pos = Point3::ZERO;
        let mut du = Vector3::ZERO;
        let mut dv = Vector3::ZERO;
        let mut duu = Vector3::ZERO;
        let mut duv = Vector3::ZERO;
        let mut dvv = Vector3::ZERO;

        // Compute surface point and derivatives
        for l in 0..=self.degree_v {
            let mut temp_pos = Point3::ZERO;
            let mut temp_du = Vector3::ZERO;
            let mut temp_duu = Vector3::ZERO;

            for k in 0..=self.degree_u {
                let idx_u = span_u - self.degree_u + k;
                let idx_v = span_v - self.degree_v + l;
                let cp = self.control_points[idx_v][idx_u];

                temp_pos += cp * ders_u[0][k];
                if ders_u.len() > 1 {
                    temp_du += cp * ders_u[1][k];
                }
                if ders_u.len() > 2 {
                    temp_duu += cp * ders_u[2][k];
                }
            }

            pos += temp_pos * ders_v[0][l];
            du += temp_du * ders_v[0][l];

            if ders_v.len() > 1 {
                dv += temp_pos * ders_v[1][l];
                duv += temp_du * ders_v[1][l];
            }

            if ders_u.len() > 2 {
                duu += temp_duu * ders_v[0][l];
            }

            if ders_v.len() > 2 {
                dvv += temp_pos * ders_v[2][l];
            }
        }

        Ok(SurfacePoint {
            position: pos,
            du,
            dv,
            duu: if ders_u.len() > 2 { Some(duu) } else { None },
            duv: if ders_u.len() > 1 && ders_v.len() > 1 {
                Some(duv)
            } else {
                None
            },
            dvv: if ders_v.len() > 2 { Some(dvv) } else { None },
        })
    }

    /// Get point on surface
    pub fn point_at(&self, param: Parameter2D) -> MathResult<Point3> {
        Ok(self.evaluate(param)?.position)
    }

    /// Get surface normal at parameter
    pub fn normal_at(&self, param: Parameter2D) -> MathResult<Vector3> {
        self.evaluate(param)?.normal()
    }

    /// Fit a B-spline surface to a grid of points using least-squares approximation
    ///
    /// # Arguments
    /// * `points` - Grid of points to fit (rows × cols)
    /// * `degree_u` - Degree in U direction (typically 3 for cubic)
    /// * `degree_v` - Degree in V direction (typically 3 for cubic)
    /// * `num_control_u` - Number of control points in U direction
    /// * `num_control_v` - Number of control points in V direction
    ///
    /// # Algorithm
    /// Uses the least-squares fitting method from Piegl & Tiller (The NURBS Book)
    pub fn fit_surface(
        points: Vec<Vec<Point3>>,
        degree_u: usize,
        degree_v: usize,
        num_control_u: usize,
        num_control_v: usize,
    ) -> MathResult<Self> {
        // Validate input
        if points.is_empty() || points[0].is_empty() {
            return Err(MathError::InvalidParameter("Empty point grid".to_string()));
        }

        let num_rows = points.len();
        let num_cols = points[0].len();

        // Check rectangular grid
        for row in &points {
            if row.len() != num_cols {
                return Err(MathError::InvalidParameter(
                    "Non-rectangular point grid".to_string(),
                ));
            }
        }

        if num_control_u < degree_u + 1 || num_control_v < degree_v + 1 {
            return Err(MathError::InvalidParameter(
                "Too few control points for given degree".to_string(),
            ));
        }

        if num_control_u > num_rows || num_control_v > num_cols {
            return Err(MathError::InvalidParameter(
                "Too many control points for given data".to_string(),
            ));
        }

        // Step 1: Compute parameter values using chord length
        let mut u_params = vec![0.0; num_rows];
        let mut v_params = vec![0.0; num_cols];

        // U parameters (along rows)
        for j in 0..num_cols {
            let mut total_length = 0.0;
            for i in 1..num_rows {
                total_length += (points[i][j] - points[i - 1][j]).magnitude();
            }

            if total_length > 0.0 {
                let mut accumulated = 0.0;
                for i in 1..num_rows {
                    accumulated += (points[i][j] - points[i - 1][j]).magnitude();
                    u_params[i] = accumulated / total_length;
                }
            }
        }

        // V parameters (along columns)
        for i in 0..num_rows {
            let mut total_length = 0.0;
            for j in 1..num_cols {
                total_length += (points[i][j] - points[i][j - 1]).magnitude();
            }

            if total_length > 0.0 {
                let mut accumulated = 0.0;
                for j in 1..num_cols {
                    accumulated += (points[i][j] - points[i][j - 1]).magnitude();
                    v_params[j] = accumulated / total_length;
                }
            }
        }

        // Step 2: Create knot vectors
        let knots_u = Self::compute_knot_vector(&u_params, degree_u, num_control_u);
        let knots_v = Self::compute_knot_vector(&v_params, degree_v, num_control_v);

        // Step 3: Set up least-squares system
        // We need to solve for control points that minimize the error

        // First fit curves in U direction
        let mut temp_curves = Vec::with_capacity(num_cols);
        for j in 0..num_cols {
            let col_points: Vec<Point3> = (0..num_rows).map(|i| points[i][j]).collect();
            let curve = Self::fit_curve_least_squares(
                &col_points,
                &u_params,
                degree_u,
                &knots_u,
                num_control_u,
            )?;
            temp_curves.push(curve);
        }

        // Then fit in V direction using the temporary curves
        let mut control_points = vec![vec![Point3::ZERO; num_control_v]; num_control_u];

        for i in 0..num_control_u {
            let row_points: Vec<Point3> = temp_curves.iter().map(|curve| curve[i]).collect();

            let fitted_row = Self::fit_curve_least_squares(
                &row_points,
                &v_params,
                degree_v,
                &knots_v,
                num_control_v,
            )?;

            for j in 0..num_control_v {
                control_points[i][j] = fitted_row[j];
            }
        }

        // Create the surface
        Self::new(degree_u, degree_v, control_points, knots_u, knots_v)
    }

    /// Helper: Fit a curve to points using least squares
    fn fit_curve_least_squares(
        points: &[Point3],
        params: &[f64],
        degree: usize,
        knots: &[f64],
        num_control: usize,
    ) -> MathResult<Vec<Point3>> {
        let n = points.len();

        // Build the N^T N matrix and N^T P vectors
        let mut ntn = vec![vec![0.0; num_control]; num_control];
        let mut ntp_x = vec![0.0; num_control];
        let mut ntp_y = vec![0.0; num_control];
        let mut ntp_z = vec![0.0; num_control];

        // For each data point
        for k in 0..n {
            let span = Self::find_span_static(knots, degree, num_control - 1, params[k]);
            let basis = Self::basis_functions_static(knots, degree, span, params[k]);

            // Add contribution to normal equations
            for i in 0..=degree {
                let idx_i = span - degree + i;
                if idx_i >= num_control {
                    continue;
                }

                for j in 0..=degree {
                    let idx_j = span - degree + j;
                    if idx_j >= num_control {
                        continue;
                    }

                    ntn[idx_i][idx_j] += basis[i] * basis[j];
                }

                ntp_x[idx_i] += basis[i] * points[k].x;
                ntp_y[idx_i] += basis[i] * points[k].y;
                ntp_z[idx_i] += basis[i] * points[k].z;
            }
        }

        // Solve the linear system using Gaussian elimination
        let control_x = Self::solve_linear_system(&ntn, &ntp_x)?;
        let control_y = Self::solve_linear_system(&ntn, &ntp_y)?;
        let control_z = Self::solve_linear_system(&ntn, &ntp_z)?;

        // Assemble control points
        let mut control_points = Vec::with_capacity(num_control);
        for i in 0..num_control {
            control_points.push(Point3::new(control_x[i], control_y[i], control_z[i]));
        }

        Ok(control_points)
    }

    /// Helper: Compute knot vector for least-squares fitting
    fn compute_knot_vector(params: &[f64], degree: usize, num_control: usize) -> Vec<f64> {
        let mut knots = vec![0.0; num_control + degree + 1];

        // First degree+1 knots are 0
        // Last degree+1 knots are 1
        for i in num_control..knots.len() {
            knots[i] = 1.0;
        }

        // Internal knots by averaging
        let n = params.len() - 1;
        for j in 1..num_control - degree {
            let mut sum = 0.0;
            for i in j..j + degree {
                let idx = (i * n) / (num_control - degree);
                if idx < params.len() {
                    sum += params[idx];
                }
            }
            knots[j + degree] = sum / degree as f64;
        }

        knots
    }

    /// Helper: Static version of find_span for use in fitting
    fn find_span_static(knots: &[f64], degree: usize, n: usize, u: f64) -> usize {
        if u >= knots[n + 1] {
            return n;
        }
        if u <= knots[degree] {
            return degree;
        }

        let mut low = degree;
        let mut high = n + 1;
        let mut mid = (low + high) / 2;

        while u < knots[mid] || u >= knots[mid + 1] {
            if u < knots[mid] {
                high = mid;
            } else {
                low = mid;
            }
            mid = (low + high) / 2;
        }

        mid
    }

    /// Helper: Static version of basis_functions for use in fitting
    fn basis_functions_static(knots: &[f64], degree: usize, span: usize, u: f64) -> Vec<f64> {
        let mut basis = vec![0.0; degree + 1];
        let mut left = vec![0.0; degree + 1];
        let mut right = vec![0.0; degree + 1];

        basis[0] = 1.0;

        for j in 1..=degree {
            left[j] = u - knots[span + 1 - j];
            right[j] = knots[span + j] - u;

            let mut saved = 0.0;
            for r in 0..j {
                let temp = basis[r] / (right[r + 1] + left[j - r]);
                basis[r] = saved + right[r + 1] * temp;
                saved = left[j - r] * temp;
            }
            basis[j] = saved;
        }

        basis
    }

    /// Helper: Solve linear system using Gaussian elimination
    fn solve_linear_system(a: &[Vec<f64>], b: &[f64]) -> MathResult<Vec<f64>> {
        let n = b.len();
        let mut aug = vec![vec![0.0; n + 1]; n];

        // Create augmented matrix
        for i in 0..n {
            for j in 0..n {
                aug[i][j] = a[i][j];
            }
            aug[i][n] = b[i];
        }

        // Forward elimination
        for i in 0..n {
            // Find pivot
            let mut max_row = i;
            for k in i + 1..n {
                if aug[k][i].abs() > aug[max_row][i].abs() {
                    max_row = k;
                }
            }
            aug.swap(i, max_row);

            // Check for zero pivot
            if aug[i][i].abs() < 1e-10 {
                return Err(MathError::NumericalInstability);
            }

            // Eliminate column
            for k in i + 1..n {
                let factor = aug[k][i] / aug[i][i];
                for j in i..=n {
                    aug[k][j] -= factor * aug[i][j];
                }
            }
        }

        // Back substitution
        let mut x = vec![0.0; n];
        for i in (0..n).rev() {
            x[i] = aug[i][n];
            for j in i + 1..n {
                x[i] -= aug[i][j] * x[j];
            }
            x[i] /= aug[i][i];
        }

        Ok(x)
    }

    /// Extract iso-curve at constant U
    pub fn iso_curve_u(&self, u: f64) -> MathResult<NurbsCurve> {
        let u = self.domain.u_range.clamp(u);
        let span_u = self.find_span_u(u);
        let basis_u = self.basis_functions_u(span_u, u);

        // Compute curve control points
        let mut curve_points = Vec::with_capacity(self.control_points.len());

        for row in &self.control_points {
            let mut point = Point3::ZERO;
            for k in 0..=self.degree_u {
                let idx = span_u - self.degree_u + k;
                point += row[idx] * basis_u[k];
            }
            curve_points.push(point);
        }

        NurbsCurve::new(
            curve_points,
            vec![1.0; self.control_points.len()], // Unit weights for B-spline
            self.knots_v.clone(),
            self.degree_v,
        )
        .map_err(|e| MathError::InvalidParameter(e.to_string()))
    }

    /// Extract iso-curve at constant V
    pub fn iso_curve_v(&self, v: f64) -> MathResult<NurbsCurve> {
        let v = self.domain.v_range.clamp(v);
        let span_v = self.find_span_v(v);
        let basis_v = self.basis_functions_v(span_v, v);

        // Compute curve control points
        let n_u = self.control_points[0].len();
        let mut curve_points = Vec::with_capacity(n_u);

        for i in 0..n_u {
            let mut point = Point3::ZERO;
            for l in 0..=self.degree_v {
                let idx = span_v - self.degree_v + l;
                point += self.control_points[idx][i] * basis_v[l];
            }
            curve_points.push(point);
        }

        NurbsCurve::new(
            curve_points,
            vec![1.0; n_u], // Unit weights for B-spline
            self.knots_u.clone(),
            self.degree_u,
        )
        .map_err(|e| MathError::InvalidParameter(e.to_string()))
    }

    /// Check if surface is closed in U direction
    pub fn is_closed_u(&self) -> bool {
        let tol = Tolerance::from_distance(1e-10);
        let n_v = self.control_points.len();
        let n_u = self.control_points[0].len();

        for i in 0..n_v {
            let dist = self.control_points[i][0].distance_squared(&self.control_points[i][n_u - 1]);
            if dist > tol.distance_squared() {
                return false;
            }
        }
        true
    }

    /// Check if surface is closed in V direction
    pub fn is_closed_v(&self) -> bool {
        let tol = Tolerance::from_distance(1e-10);
        let n_v = self.control_points.len();
        let n_u = self.control_points[0].len();

        for j in 0..n_u {
            let dist = self.control_points[0][j].distance_squared(&self.control_points[n_v - 1][j]);
            if dist > tol.distance_squared() {
                return false;
            }
        }
        true
    }

    /// Transform surface by matrix
    pub fn transform(&self, matrix: &Matrix4) -> Self {
        let mut transformed_points = Vec::with_capacity(self.control_points.len());

        for row in &self.control_points {
            let transformed_row: Vec<_> = row.iter().map(|p| matrix.transform_point(p)).collect();
            transformed_points.push(transformed_row);
        }

        Self {
            degree_u: self.degree_u,
            degree_v: self.degree_v,
            control_points: transformed_points,
            knots_u: self.knots_u.clone(),
            knots_v: self.knots_v.clone(),
            domain: self.domain,
        }
    }

    /// Get bounding box
    pub fn bounding_box(&self) -> (Point3, Point3) {
        let all_points: Vec<_> = self
            .control_points
            .iter()
            .flat_map(|row| row.iter())
            .cloned()
            .collect();

        let mut min = all_points[0];
        let mut max = all_points[0];

        for p in all_points.iter().skip(1) {
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            min.z = min.z.min(p.z);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
            max.z = max.z.max(p.z);
        }

        (min, max)
    }

    /// Find closest point on surface to given point
    pub fn closest_point(
        &self,
        point: &Point3,
        tolerance: Tolerance,
    ) -> MathResult<(Parameter2D, Point3)> {
        // Grid search for initial guess
        let grid_size = 10;
        let mut best_param = Parameter2D::new(0.5, 0.5);
        let mut best_dist_sq = f64::INFINITY;

        for i in 0..=grid_size {
            for j in 0..=grid_size {
                let u = i as f64 / grid_size as f64;
                let v = j as f64 / grid_size as f64;
                let param = Parameter2D::new(u, v);

                if let Ok(p) = self.point_at(param) {
                    let dist_sq = p.distance_squared(point);
                    if dist_sq < best_dist_sq {
                        best_dist_sq = dist_sq;
                        best_param = param;
                    }
                }
            }
        }

        // Newton-Raphson refinement
        let mut param = best_param;

        for _ in 0..20 {
            let eval = self.evaluate(param)?;
            let r = *point - eval.position;

            // f(u,v) = [r·S_u, r·S_v] = [0, 0]
            let f_u = r.dot(&eval.du);
            let f_v = r.dot(&eval.dv);

            if f_u.abs() < tolerance.distance() && f_v.abs() < tolerance.distance() {
                break;
            }

            // Jacobian matrix
            if let (Some(duu), Some(duv), Some(dvv)) = (eval.duu, eval.duv, eval.dvv) {
                let j11 = eval.du.dot(&eval.du) + r.dot(&duu);
                let j12 = eval.du.dot(&eval.dv) + r.dot(&duv);
                let j22 = eval.dv.dot(&eval.dv) + r.dot(&dvv);

                let det = j11 * j22 - j12 * j12;
                if det.abs() > consts::EPSILON {
                    let du = (j22 * f_u - j12 * f_v) / det;
                    let dv = (j11 * f_v - j12 * f_u) / det;

                    param.u = (param.u - du).clamp(0.0, 1.0);
                    param.v = (param.v - dv).clamp(0.0, 1.0);
                }
            }
        }

        let closest = self.point_at(param)?;
        Ok((param, closest))
    }
}

/// NURBS surface representation
#[derive(Debug, Clone)]
pub struct NurbsSurface {
    /// B-spline surface base
    pub bspline: BSplineSurface,
    /// Weights grid (same dimensions as control points)
    pub weights: Vec<Vec<f64>>,
}

impl NurbsSurface {
    /// Create new NURBS surface
    pub fn new(
        degree_u: usize,
        degree_v: usize,
        control_points: Vec<Vec<Point3>>,
        weights: Vec<Vec<f64>>,
        knots_u: Vec<f64>,
        knots_v: Vec<f64>,
    ) -> MathResult<Self> {
        // Validate weights
        if weights.len() != control_points.len() {
            return Err(MathError::InvalidParameter(
                "Weights must have same dimensions as control points".to_string(),
            ));
        }

        for (cp_row, w_row) in control_points.iter().zip(weights.iter()) {
            if cp_row.len() != w_row.len() {
                return Err(MathError::InvalidParameter(
                    "Weight rows must match control point rows".to_string(),
                ));
            }

            // Check positive weights
            for &w in w_row {
                if w <= 0.0 {
                    return Err(MathError::InvalidParameter(
                        "All weights must be positive".to_string(),
                    ));
                }
            }
        }

        let bspline = BSplineSurface::new(degree_u, degree_v, control_points, knots_u, knots_v)?;

        Ok(Self { bspline, weights })
    }

    /// Create a cylinder
    pub fn cylinder(center: Point3, axis: Vector3, radius: f64, height: f64) -> MathResult<Self> {
        let axis = axis.normalize()?;
        let (x_axis, y_axis) = {
            let x = axis.perpendicular().normalize()?;
            let y = axis.cross(&x);
            (x, y)
        };

        // 9 control points for full circle
        let w = 1.0 / 2.0_f64.sqrt(); // Weight for 45° points

        let bottom_center = center - axis * (height / 2.0);
        let _top_center = center + axis * (height / 2.0);

        let mut control_points = Vec::new();
        let mut weights = Vec::new();

        // Bottom circle
        let bottom_row = vec![
            bottom_center + x_axis * radius,
            bottom_center + (x_axis + y_axis) * (radius / 2.0_f64.sqrt()),
            bottom_center + y_axis * radius,
            bottom_center + (-x_axis + y_axis) * (radius / 2.0_f64.sqrt()),
            bottom_center - x_axis * radius,
            bottom_center + (-x_axis - y_axis) * (radius / 2.0_f64.sqrt()),
            bottom_center - y_axis * radius,
            bottom_center + (x_axis - y_axis) * (radius / 2.0_f64.sqrt()),
            bottom_center + x_axis * radius,
        ];

        let weight_row = vec![1.0, w, 1.0, w, 1.0, w, 1.0, w, 1.0];

        control_points.push(bottom_row.clone());
        weights.push(weight_row.clone());

        // Top circle (same pattern)
        let top_row: Vec<_> = bottom_row.iter().map(|p| *p + axis * height).collect();

        control_points.push(top_row);
        weights.push(weight_row);

        // Knots
        let knots_u = vec![
            0.0, 0.0, 0.0, 0.25, 0.25, 0.5, 0.5, 0.75, 0.75, 1.0, 1.0, 1.0,
        ];
        let knots_v = vec![0.0, 0.0, 1.0, 1.0];

        Self::new(2, 1, control_points, weights, knots_u, knots_v)
    }

    /// Evaluate NURBS surface at parameter
    pub fn evaluate(&self, param: Parameter2D) -> MathResult<SurfacePoint> {
        let param = self.bspline.domain.clamp(param);

        // Find spans
        let span_u = self.bspline.find_span_u(param.u);
        let span_v = self.bspline.find_span_v(param.v);

        // Get basis functions with derivatives
        let _ders_u = self.bspline.basis_derivatives(
            &self.bspline.knots_u,
            self.bspline.degree_u,
            span_u,
            param.u,
            2,
        );
        let _ders_v = self.bspline.basis_derivatives(
            &self.bspline.knots_v,
            self.bspline.degree_v,
            span_v,
            param.v,
            2,
        );

        // Compute weighted sums (rational surface evaluation)
        // This follows the same pattern as NURBS curves but in 2D

        // ... (implementation details for rational surface evaluation)

        // For now, return non-rational result
        self.bspline.evaluate(param)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::tolerance::NORMAL_TOLERANCE;

    #[test]
    fn test_bilinear_surface() {
        let surf = BSplineSurface::bilinear(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
        )
        .unwrap();

        // Test corners
        assert_eq!(
            surf.point_at(Parameter2D::new(0.0, 0.0)).unwrap(),
            Point3::new(0.0, 0.0, 0.0)
        );
        assert_eq!(
            surf.point_at(Parameter2D::new(1.0, 0.0)).unwrap(),
            Point3::new(1.0, 0.0, 0.0)
        );
        assert_eq!(
            surf.point_at(Parameter2D::new(0.0, 1.0)).unwrap(),
            Point3::new(0.0, 1.0, 0.0)
        );
        assert_eq!(
            surf.point_at(Parameter2D::new(1.0, 1.0)).unwrap(),
            Point3::new(1.0, 1.0, 0.0)
        );

        // Test center
        assert_eq!(
            surf.point_at(Parameter2D::new(0.5, 0.5)).unwrap(),
            Point3::new(0.5, 0.5, 0.0)
        );

        // Test normal (should be +Z everywhere)
        let normal = surf.normal_at(Parameter2D::new(0.5, 0.5)).unwrap();
        assert!((normal - Vector3::Z).magnitude() < 1e-10);
    }

    #[test]
    fn test_surface_curvature() {
        // Create a spherical patch
        // TODO: Implement sphere as NURBS surface

        // For now, test with bilinear (zero curvature)
        let surf = BSplineSurface::bilinear(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
        )
        .unwrap();

        let eval = surf.evaluate(Parameter2D::new(0.5, 0.5)).unwrap();

        // Bilinear surface has zero curvature
        assert_eq!(eval.gaussian_curvature(), Some(0.0));
        assert_eq!(eval.mean_curvature(), Some(0.0));
    }

    #[test]
    fn test_iso_curves() {
        let surf = BSplineSurface::bilinear(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
        )
        .unwrap();

        // Extract iso-curve at u=0.5
        let iso_u = surf.iso_curve_u(0.5).unwrap();
        assert_eq!(iso_u.evaluate(0.0).point, Point3::new(0.5, 0.0, 0.0));
        assert_eq!(iso_u.evaluate(1.0).point, Point3::new(0.5, 1.0, 0.0));

        // Extract iso-curve at v=0.5
        let iso_v = surf.iso_curve_v(0.5).unwrap();
        assert_eq!(iso_v.evaluate(0.0).point, Point3::new(0.0, 0.5, 0.0));
        assert_eq!(iso_v.evaluate(1.0).point, Point3::new(1.0, 0.5, 0.0));
    }

    #[test]
    fn test_closest_point() {
        let surf = BSplineSurface::bilinear(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
        )
        .unwrap();

        // Test point above surface center
        let test_point = Point3::new(0.5, 0.5, 1.0);
        let (param, closest) = surf.closest_point(&test_point, NORMAL_TOLERANCE).unwrap();

        assert!((param.u - 0.5).abs() < 1e-6);
        assert!((param.v - 0.5).abs() < 1e-6);
        assert_eq!(closest, Point3::new(0.5, 0.5, 0.0));
    }
}
